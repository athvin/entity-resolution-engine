//! The clear endpoint (batch-orchestration-design §5.4): retry ONE task of a
//! job without re-running the whole job, over the real route.
//!
//! `pg_task_instances.rs` Phase E proves the clear STATEMENTS. This file
//! proves the HTTP surface on top of them: that the closure is the server's to
//! compute from the compiled graph rather than the caller's to assert, that an
//! unknown task id is a 404 rather than a silent no-op reported as success,
//! and that a clear under a cancel is refused.
//!
//! One `#[tokio::test]` with sequential phases (`pg_serial.rs`).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use er_server::dags::{self, JobKind};
use er_server::{AppState, app, pg, taskpg};

use crate::common;

const EPOCH: &str = "epoch-clear";

/// A request as a real caller sends it: the `content-type` header rides ONLY
/// with a body. That is what makes `Option<Json<_>>` resolve to `None` for a
/// bodyless clear rather than failing to deserialize an empty string — and it
/// is what the BFF's `forward` does, so this mirrors production.
fn op(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let b = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", "Bearer op-secret");
    match body {
        Some(v) => b
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn the_clear_endpoint_computes_its_own_closure() {
    let Some(db) = common::fresh_db("clear_endpoint").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::ensure_org(&client, "acme", "/tmp/acme.yaml")
        .await
        .unwrap();
    let org_id: String = client
        .query_one("SELECT id FROM orgs WHERE name = 'acme'", &[])
        .await
        .unwrap()
        .get(0);
    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    // A planned run_all_full: seven rows, none of them started.
    let dag = dags::dag_for(JobKind::RunAllFull);
    let job = pg::enqueue(
        &client,
        &org_id,
        None,
        "run_all_full",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    let claimed = pg::claim(&client, 1).await.unwrap().expect("claimable");
    taskpg::plan(&mut client, &claimed, &dag, EPOCH, None)
        .await
        .unwrap()
        .expect("plannable");

    let base = format!("/v1/orgs/acme/datasets/default/jobs/{}", job.id);

    // ---- phase 1: an unknown task id is a 404 ------------------------------
    //
    // Not a 200 with an empty closure: a typo must say so, or an operator
    // believes they retried something they did not.
    let resp = app
        .clone()
        .oneshot(op("POST", &format!("{base}/tasks/nosuchtask/clear"), None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let detail = body_json(resp).await;
    assert!(
        detail["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("is not a task of kind 'run_all_full'"),
        "the refusal names the kind: {detail}"
    );

    // ---- phase 2: the server computes the closure from the graph -----------
    //
    // Clearing `standardize` must also clear everything downstream of it —
    // re-running a stage without re-running what consumed it would leave the
    // lake describing two different runs. The caller asked for one task.
    let resp = app
        .clone()
        .oneshot(op("POST", &format!("{base}/tasks/standardize/clear"), None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let cleared = body_json(resp).await;
    let names: Vec<&str> = cleared["cleared"]
        .as_array()
        .expect("cleared is a list")
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect();
    for want in ["standardize", "match", "reconcile", "assemble", "finish"] {
        assert!(names.contains(&want), "{want} is in the closure: {names:?}");
    }
    for never in ["init", "ingest"] {
        assert!(
            !names.contains(&never),
            "{never} is UPSTREAM and must not be cleared: {names:?}"
        );
    }
    // Every row was pending, so all three clear arms report the pending one.
    assert_eq!(cleared["restarting"], 0, "nothing was live");
    assert_eq!(cleared["rotated"], 0, "nothing was terminal");
    assert_eq!(
        cleared["demanded_upstream"],
        json!([]),
        "this process cannot probe a lake, so it drafts in no upstream"
    );

    // ---- phase 3: a body may widen the budget ------------------------------
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            &format!("{base}/tasks/assemble/clear"),
            Some(json!({ "retries": 2, "resource_class": "L" })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let rows = er_entities::task_instance::read::by_job(&client, &org_id, &job.id)
        .await
        .unwrap();
    let assemble = rows
        .iter()
        .find(|r| r.task_id == "assemble" && r.live == Some(true))
        .expect("the assemble row");
    assert_eq!(
        assemble.resource_class.as_deref(),
        Some("L"),
        "the operator's escalation landed on the row"
    );

    // ---- phase 4: an unknown field is refused -------------------------------
    //
    // `deny_unknown_fields`, so a caller who misspells `retries` is told,
    // rather than silently getting the default.
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            &format!("{base}/tasks/assemble/clear"),
            Some(json!({ "retires": 2 })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "`deny_unknown_fields` rejects a misspelled field rather than \
         silently applying the default"
    );

    // ---- phase 5: a clear under a cancel is refused -------------------------
    let canceled = er_entities::job::update::cancel(&client, &job.id, &org_id)
        .await
        .unwrap();
    assert_eq!(canceled[0].state, "canceling");
    let resp = app
        .clone()
        .oneshot(op("POST", &format!("{base}/tasks/assemble/clear"), None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "a clear would hand new work to a job whose pods are being torn down"
    );

    // ---- phase 6: viewer may not clear --------------------------------------
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{base}/tasks/assemble/clear"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "clearing is a steward act, and an unauthenticated caller is not one"
    );
}
