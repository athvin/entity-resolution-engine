//! The remote-execution fence (#1133) against real Postgres: the attempt-token
//! journal derives its scope from the authenticated task, refuses a stale pod,
//! journals COMMIT at most once, holds the dataset against every writer until
//! the outcome is established, and the reconciliation sweep releases only on
//! conclusive evidence. One `#[tokio::test]` with sequential phases.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use er_duckfleet::Uuid;
use er_duckfleet::execution::recovery::{ReadFailed, RequestOutcome, RequestState};
use er_server::dags::{self, JobKind};
use er_server::remote_attempts::{Gatherer, sweep_with};
use er_server::{AppState, pg, taskpg};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::common;

const EPOCH: &str = "epoch-1";

async fn remote(
    app: &axum::Router,
    attempt: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/internal/attempts/{attempt}/remote"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn fixed(receipt: Result<bool, ReadFailed>, commit: Result<RequestState, ReadFailed>) -> Gatherer {
    Arc::new(move |_, _| (receipt, commit))
}

#[tokio::test]
#[expect(clippy::too_many_lines, reason = "one test, sequential phases")]
async fn the_remote_fence_holds_until_evidence_settles_it() {
    let Some(db) = common::fresh_db("remote_attempt_fence").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::ensure_org(&client, "acme", "/tmp/acme.yaml")
        .await
        .unwrap();
    pg::converge_default_datasets(&mut client).await.unwrap();
    let org_id = pg::org_id_by_name(&client, "acme").await.unwrap().unwrap();
    let dataset = common::default_dataset(&client, "acme").await;

    // ---- A running task on the dataset, started as pod-golden --------------
    let dag = dags::dag_for(JobKind::RunAllFull);
    let job = pg::enqueue(
        &client,
        &org_id,
        Some(&dataset),
        "run_all_full",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    let claimed = pg::claim(&client, 1).await.unwrap().expect("claimable");
    taskpg::plan(&mut client, &claimed, &dag, EPOCH, Some("sha256:test"))
        .await
        .unwrap()
        .expect("plannable");
    let task = taskpg::attempt_id(&job.id, "init", 1);
    er_entities::task_instance::update::mark_scheduled(
        &client,
        &json!({"pins": [], "params": {}}),
        &task,
        0,
    )
    .await
    .unwrap();
    let token = "tok-golden-1";
    let job_name = format!("er-t-{}-l1", &task[..20]);
    taskpg::claim_launch(
        &client,
        EPOCH,
        Some(&job_name),
        Some(&pg::hash_secret(token)),
        1,
    )
    .await
    .unwrap()
    .expect("launchable");
    let app = er_server::app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/internal/attempts/{task}/start"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"k8s_job_name": job_name, "pod_uid": "pod-golden"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // ---- Phase A: scope comes from the token, identifiers only ------------
    let first = Uuid::now_v7().to_string();
    let (status, _) = remote(
        &app,
        &task,
        "not-the-token",
        json!({"op":"acquire","id":first,"pod_uid":"pod-golden","binding_generation":1,"namespace":"ns"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = remote(
        &app,
        &task,
        token,
        json!({"op":"acquire","id":"DROP TABLE x","pod_uid":"pod-golden","binding_generation":1,"namespace":"ns"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // A body cannot name another org or dataset: the fields do not exist.
    let (status, _) = remote(
        &app,
        &task,
        token,
        json!({"op":"acquire","id":first,"pod_uid":"pod-golden","binding_generation":1,
               "namespace":"ns","dataset_id":"elsewhere"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // A stale pod (not the registered one) is refused.
    let (status, _) = remote(
        &app,
        &task,
        token,
        json!({"op":"acquire","id":first,"pod_uid":"pod-old","binding_generation":1,"namespace":"ns"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, body) = remote(
        &app,
        &task,
        token,
        json!({"op":"acquire","id":first,"pod_uid":"pod-golden","binding_generation":1,"namespace":"ns"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = er_entities::remote_attempt::read::by_id(&client, &first)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(
        (row.org_id.as_str(), row.dataset_id.as_str()),
        (org_id.as_str(), dataset.as_str())
    );
    assert_eq!(
        (row.job_id.as_str(), row.launch, row.phase.as_str()),
        (job.id.as_str(), 1, "opening")
    );

    // One unresolved attempt per dataset: a second is refused, and the
    // refusal names the holder without anything from the service.
    let second = Uuid::now_v7().to_string();
    let (status, body) = remote(
        &app,
        &task,
        token,
        json!({"op":"acquire","id":second,"pod_uid":"pod-golden","binding_generation":1,"namespace":"ns"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["detail"].as_str().unwrap().contains(&first), "{body}");

    // ---- Phase B: the journal, then an owner release (no COMMIT sent) -----
    let session = Uuid::now_v7().to_string();
    let request = |id: &str, commit: bool, pod: &str| json!({"op":"request","id":id,"pod_uid":pod,"request_id":Uuid::now_v7().to_string(),"commit":commit});
    // No request before the session is recorded.
    assert_eq!(
        remote(&app, &task, token, request(&first, false, "pod-golden"))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let (status, _) = remote(
        &app,
        &task,
        token,
        json!({"op":"session","id":first,"pod_uid":"pod-golden","session_id":session}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        remote(&app, &task, token, request(&first, false, "pod-golden"))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        remote(&app, &task, token, request(&first, false, "pod-old"))
            .await
            .0,
        StatusCode::CONFLICT
    );
    // `unknown` is never a release phase.
    let (status, _) = remote(
        &app,
        &task,
        token,
        json!({"op":"release","id":first,"phase":"unknown"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = remote(
        &app,
        &task,
        token,
        json!({"op":"release","id":first,"phase":"rolled_back"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = remote(
        &app,
        &task,
        token,
        json!({"op":"release","id":first,"phase":"committed"}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a released attempt is never rewritten"
    );

    // One remote attempt per launch: even released, this launch cannot start
    // another, so a restarted pod never inherits a prior session.
    let again = Uuid::now_v7().to_string();
    let (status, _) = remote(
        &app,
        &task,
        token,
        json!({"op":"acquire","id":again,"pod_uid":"pod-golden","binding_generation":1,"namespace":"ns"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // ---- Phase C: a relaunch whose journaled COMMIT went unsettled --------
    client
        .execute(
            "UPDATE task_instances SET launches = 2, pod_uid = 'pod-relaunch' WHERE id = $1",
            &[&task],
        )
        .await
        .unwrap();
    // The previous launch's pod is fenced out of everything.
    let (status, _) = remote(
        &app,
        &task,
        token,
        json!({"op":"acquire","id":again,"pod_uid":"pod-golden","binding_generation":1,"namespace":"ns"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let held = Uuid::now_v7().to_string();
    for body in [
        json!({"op":"acquire","id":held,"pod_uid":"pod-relaunch","binding_generation":1,"namespace":"ns"}),
        json!({"op":"session","id":held,"pod_uid":"pod-relaunch","session_id":Uuid::now_v7().to_string()}),
        request(&held, false, "pod-relaunch"),
        request(&held, true, "pod-relaunch"),
    ] {
        let (status, reply) = remote(&app, &task, token, body).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
    }
    assert_eq!(
        remote(&app, &task, token, request(&held, true, "pod-relaunch"))
            .await
            .0,
        StatusCode::CONFLICT,
        "COMMIT is journaled at most once"
    );
    assert_eq!(
        remote(&app, &task, token, json!({"op":"unknown","id":held}))
            .await
            .0,
        StatusCode::OK
    );
    // While the owner lives, the sweep never settles it, even with a receipt
    // that would be conclusive: the working set excludes live owners.
    let calls = Arc::new(AtomicUsize::new(0));
    let counting: Gatherer = {
        let calls = Arc::clone(&calls);
        Arc::new(move |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            (Ok(true), Ok(RequestState::Pending))
        })
    };
    assert_eq!(
        sweep_with(&client, Duration::ZERO, Some(counting))
            .await
            .unwrap(),
        0
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    // The pod is lost and the job fails while the COMMIT is unsettled.
    client
        .execute(
            "UPDATE task_instances SET state = 'failed', live = false WHERE id = $1",
            &[&task],
        )
        .await
        .unwrap();
    client
        .execute(
            "UPDATE jobs SET state = 'failed', finished_at = now() WHERE id = $1",
            &[&job.id],
        )
        .await
        .unwrap();
    // A stale pod can no longer journal anything.
    assert_eq!(
        remote(&app, &task, token, request(&held, false, "pod-relaunch"))
            .await
            .0,
        StatusCode::CONFLICT
    );

    // ---- Phase D: every writer is held, whatever its backend --------------
    let retry = pg::enqueue(
        &client,
        &org_id,
        Some(&dataset),
        "run_all_full",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    assert!(
        pg::claim(&client, 5).await.unwrap().is_none(),
        "the dataset is fenced"
    );
    client
        .execute(
            "UPDATE jobs SET state = 'canceled', finished_at = now() WHERE id = $1",
            &[&retry.id],
        )
        .await
        .unwrap();
    let org_job = pg::enqueue(
        &client,
        &org_id,
        None,
        "provision",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    assert!(
        pg::claim(&client, 5).await.unwrap().is_none(),
        "an org-level job waits too"
    );
    client
        .execute(
            "UPDATE jobs SET state = 'canceled', finished_at = now() WHERE id = $1",
            &[&org_job.id],
        )
        .await
        .unwrap();
    let retry = pg::enqueue(
        &client,
        &org_id,
        Some(&dataset),
        "run_all_full",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();

    // ---- Phase E: the sweep releases only on conclusive evidence ----------
    let unknown = || Ok(RequestState::Terminal(RequestOutcome::Unknown));
    for (gather, why) in [
        (None, "no recovery binding configured"),
        (
            Some(fixed(Err(ReadFailed), unknown())),
            "receipt unreadable",
        ),
        (
            Some(fixed(Ok(false), Err(ReadFailed))),
            "commit record unreadable",
        ),
        (
            Some(fixed(Ok(false), Ok(RequestState::Pending))),
            "commit still executing",
        ),
        (
            Some(fixed(
                Ok(false),
                Ok(RequestState::Terminal(RequestOutcome::StatementResult)),
            )),
            "success without a receipt",
        ),
    ] {
        assert_eq!(
            sweep_with(&client, Duration::ZERO, gather).await.unwrap(),
            0,
            "{why}"
        );
    }
    // Conclusive, but inside the delivery grace.
    assert_eq!(
        sweep_with(
            &client,
            Duration::from_secs(3600),
            Some(fixed(Ok(false), unknown()))
        )
        .await
        .unwrap(),
        0
    );
    assert!(
        pg::claim(&client, 5).await.unwrap().is_none(),
        "still fenced"
    );
    // Grace elapsed: a finished COMMIT with no receipt is a rollback.
    assert_eq!(
        sweep_with(&client, Duration::ZERO, Some(fixed(Ok(false), unknown())))
            .await
            .unwrap(),
        1
    );
    let row = er_entities::remote_attempt::read::by_id(&client, &held)
        .await
        .unwrap()
        .remove(0);
    assert_eq!((row.phase.as_str(), row.launch), ("rolled_back", 2));
    assert!(row.released_at.is_some());
    assert!(row.commit_request_id.is_some());
    let next = pg::claim(&client, 5).await.unwrap().expect("released");
    assert_eq!(
        next.id, retry.id,
        "the retry runs only after reconciliation"
    );
    // Nothing left to reconcile; a further sweep is a no-op.
    assert_eq!(
        sweep_with(&client, Duration::ZERO, Some(fixed(Ok(true), unknown())))
            .await
            .unwrap(),
        0
    );
}
