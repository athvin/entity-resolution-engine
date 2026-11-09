//! The task-ledger statement choreography (batch-orchestration-design §4.5),
//! one phase per decision: planning in one transaction with the claim; the
//! readiness → claim → start → heartbeat → finish chain (driven over the
//! attempt channel's HTTP routes with the fallback token); `fail_or_retry`
//! with the exact deterministic backoff and the OOM escalation; the clear's
//! three arms, `reopen` and the `clear_number` fence; `complete_restart`;
//! cancel through `canceling` with the teardown row claimable; the control
//! claim; and a two-member segment through `finish_and_advance` with the
//! one-pod index holding at every statement boundary.
//!
//! One `#[tokio::test]` with sequential phases (`pg_serial.rs`).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use er_dag::{PassAction, ReferenceExistence, SettleVerdict, TaskState, clear_closure, settle};
use er_server::dags::{self, JobKind};
use er_server::{AppState, pg, taskpg};

use crate::common;

const EPOCH: &str = "epoch-1";

async fn states_for(
    client: &tokio_postgres::Client,
    dag: &er_dag::Dag,
    org_id: &str,
    job_id: &str,
) -> Vec<TaskState> {
    let rows = er_entities::task_instance::read::by_job(client, org_id, job_id)
        .await
        .unwrap();
    dag.nodes
        .iter()
        .map(|n| {
            let live = rows
                .iter()
                .find(|r| r.task_id == n.task_id && r.live == Some(true))
                .expect("a live row per node");
            TaskState::parse(&live.state).expect("a declared state")
        })
        .collect()
}

fn bearer(req: axum::http::request::Builder, token: &str) -> axum::http::request::Builder {
    req.header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn the_task_ledger_statements_hold_their_contracts() {
    let Some(db) = common::fresh_db("task_instances").await else {
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

    // ---- Phase A: planning is one transaction with the claim (§4.5) -------
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
    assert_eq!(claimed.state, "dispatching");
    let planned = taskpg::plan(&mut client, &claimed, &dag, EPOCH, Some("sha256:test"))
        .await
        .unwrap()
        .expect("the claim just handed out is plannable");
    assert_eq!(planned.state, "running");
    assert_eq!(planned.run_id.as_deref(), Some(job.id.as_str()));
    assert_eq!(
        planned.structural_fingerprint.as_deref(),
        Some(dag.structural_fingerprint.as_str())
    );
    let rows = er_entities::task_instance::read::by_job(&client, &org_id, &job.id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 7, "one row per compiled node");
    assert!(rows.iter().all(|r| r.state == "none" && r.try_number == 1));
    let id_of = |task: &str| taskpg::attempt_id(&job.id, task, 1);
    for row in &rows {
        assert_eq!(row.id, taskpg::attempt_id(&job.id, &row.task_id, 1));
    }
    // The finish row is teardown and carries the all_terminal rule.
    let finish_row = rows.iter().find(|r| r.task_id == "finish").unwrap();
    assert!(finish_row.teardown);
    assert_eq!(finish_row.trigger_rule, "all_terminal");

    // The Rust mint and the SQL mint of the same attempt key agree, byte for
    // byte (the §4.5 successor-id recipe).
    let sql_mint: String = client
        .query_one(
            "SELECT encode(sha256(convert_to('task_instances:' || $1 || ':' || $2 || ':' || $3, 'UTF8')), 'hex')",
            &[&job.id.as_str(), &"ingest", &"1"],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(sql_mint, id_of("ingest"));

    // Re-planning inserts nothing (ON CONFLICT DO NOTHING on the attempt key).
    let replayed = er_entities::task_instance::create::plan(
        &client,
        &id_of("init"),
        &org_id,
        None,
        &job.id,
        "init",
        0,
        "pod",
        "all_succeeded",
        &json!([]),
        false,
        None,
        Some("M"),
        2,
        3,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(replayed.is_empty(), "the plan insert is idempotent");
    // refuse_plan only leaves `dispatching`; this job is running — a no-op.
    assert!(
        taskpg::refuse_plan(&client, &job.id, "config", "nope")
            .await
            .unwrap()
            .is_none()
    );

    // ---- Phase B: readiness → claim → start → heartbeat → finish ----------
    let states = states_for(&client, &dag, &org_id, &job.id).await;
    let actions = er_dag::pass(&dag, &states);
    assert_eq!(actions, vec![(0, PassAction::Schedule)], "only init fires");
    let init_id = id_of("init");
    let scheduled = er_entities::task_instance::update::mark_scheduled(
        &client,
        &json!({"pins": [], "params": {}}),
        &init_id,
        0,
    )
    .await
    .unwrap();
    assert_eq!(scheduled.len(), 1);
    let token = "tok-init-1";
    let job_name = format!("er-t-{}-l1", &init_id[..20]);
    let launched = taskpg::claim_launch(
        &client,
        EPOCH,
        Some(&job_name),
        Some(&pg::hash_secret(token)),
        1,
    )
    .await
    .unwrap()
    .expect("the scheduled init is claimable");
    assert_eq!(launched.id, init_id);
    assert_eq!(launched.state, "queued");
    assert_eq!(launched.launches, 1);
    // The org slot is taken: nothing else claims.
    assert!(
        taskpg::claim_launch(&client, EPOCH, Some("er-t-x-l1"), None, 10)
            .await
            .unwrap()
            .is_none()
    );

    // The pod half runs over the attempt channel with the fallback token.
    let app = er_server::app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    // A wrong token is refused before anything is read.
    let resp = app
        .clone()
        .oneshot(
            bearer(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/internal/attempts/{init_id}")),
                "not-the-token",
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let resp = app
        .clone()
        .oneshot(
            bearer(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/internal/attempts/{init_id}")),
                token,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let order = body_json(resp).await;
    assert_eq!(order["task_id"], "init");
    assert_eq!(order["run_id"], job.id);
    assert_eq!(order["kind"], "run_all_full");
    assert_eq!(order["lineage"], json!([init_id]));
    // The registry crosses the wire, because the pod cannot read it (§6.4):
    // this org has no active model, so the list is present and EMPTY — which
    // is what `runctx` falls back from, not what it fails on. Absent entirely
    // would be indistinguishable from "the server forgot".
    assert_eq!(
        order["models"],
        json!([]),
        "the resolved registry is carried"
    );
    assert!(
        er_server::runctx::models_from_order(&order["models"]).is_empty(),
        "and the resolver reads it"
    );

    let resp = app
        .clone()
        .oneshot(
            bearer(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/internal/attempts/{init_id}/start")),
                token,
            )
            .body(Body::from(
                json!({"k8s_job_name": job_name, "pod_uid": "pod-init", "pod_name": "er-t-abc", "node_name": "node-1"})
                    .to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let first_start = body_json(resp).await;
    assert_eq!(first_start["state"], "running");

    // Simulate losing that 200 after Postgres committed. The pod retries with
    // its same UID; the stored identity and timestamps must not be rewritten
    // by a changed request body.
    let resp = app
        .clone()
        .oneshot(
            bearer(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/internal/attempts/{init_id}/start")),
                token,
            )
            .body(Body::from(
                json!({"k8s_job_name": job_name, "pod_uid": "pod-init", "pod_name": "changed-name", "node_name": "changed-node", "image_digest": "sha256:changed"})
                    .to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp).await, first_start, "start replay is stable");

    for (job_name, pod_uid) in [
        (job_name.as_str(), "pod-zombie"),
        ("er-t-other-launch-l2", "pod-init"),
    ] {
        let resp = app
            .clone()
            .oneshot(
                bearer(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/v1/internal/attempts/{init_id}/start")),
                    token,
                )
                .body(Body::from(
                    json!({"k8s_job_name": job_name, "pod_uid": pod_uid}).to_string(),
                ))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT, "the start fence");
    }

    // A heartbeat from the wrong pod is the fence (409), the right one lands.
    let resp = app
        .clone()
        .oneshot(
            bearer(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/internal/attempts/{init_id}/heartbeat")),
                token,
            )
            .body(Body::from(json!({"pod_uid": "pod-zombie"}).to_string()))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT, "the fence");
    let resp = app
        .clone()
        .oneshot(
            bearer(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/internal/attempts/{init_id}/heartbeat")),
                token,
            )
            .body(Body::from(
                json!({"pod_uid": "pod-init", "counters": {"rows_in": 10}}).to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(
            bearer(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/internal/attempts/{init_id}/finish")),
                token,
            )
            .body(Body::from(
                json!({
                    "state": "succeeded", "pod_uid": "pod-init", "outcome": "done",
                    "output": {"plan": "run_plan", "doc": {"mode": "full", "run_at": "2026-01-01 00:00:00"}},
                    "snapshot_end": 1, "exit_code": 0
                })
                .to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let finished = body_json(resp).await;
    assert_eq!(finished["state"], "succeeded");
    assert_eq!(
        finished["token_hash"],
        Value::Null,
        "the token is nulled at finish"
    );
    // With the token nulled, the channel is closed for this attempt.
    let resp = app
        .clone()
        .oneshot(
            bearer(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/internal/attempts/{init_id}")),
                token,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // The viewer surface sees the ledger.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/v1/orgs/acme/datasets/default/jobs/{}/tasks",
                    job.id
                ))
                .header("authorization", "Bearer op-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let tasks = body_json(resp).await;
    assert_eq!(tasks["tasks"].as_array().unwrap().len(), 7);

    // ---- Phase C: fail_or_retry — exact backoff, lineage, escalation ------
    let ingest1 = id_of("ingest");
    er_entities::task_instance::update::mark_scheduled(&client, &json!({}), &ingest1, 0)
        .await
        .unwrap();
    let ingest_job1 = format!("er-t-{}-l1", &ingest1[..20]);
    let ingest_token = "tok-ingest-1";
    taskpg::claim_launch(
        &client,
        EPOCH,
        Some(&ingest_job1),
        Some(&pg::hash_secret(ingest_token)),
        1,
    )
    .await
    .unwrap()
    .expect("ingest try 1 claims");
    er_entities::task_instance::update::start(
        &client,
        None,
        "pod-i1",
        None,
        None,
        &ingest1,
        &ingest_job1,
    )
    .await
    .unwrap();

    // A stale archive (wrong launch identity) is a no-op, never an error.
    let stale = taskpg::fail_or_retry_task(
        &mut client,
        &ingest1,
        &taskpg::TaskFailure {
            terminal_state: "failed",
            retryable: true,
            escalate: false,
            exit_code: Some(1),
            termination_reason: None,
            error_class: Some("transient_io"),
            error_detail: Some("lost the launch race"),
            k8s_job_name: Some("er-t-someone-else-l9"),
            backoff: taskpg::POD_BACKOFF,
        },
    )
    .await
    .unwrap();
    assert_eq!(stale, (None, None));

    let (archived, retry) = taskpg::fail_or_retry_task(
        &mut client,
        &ingest1,
        &taskpg::TaskFailure {
            terminal_state: "failed",
            retryable: true,
            escalate: false,
            exit_code: Some(1),
            termination_reason: None,
            error_class: Some("transient_io"),
            error_detail: Some("open lake: connection reset"),
            k8s_job_name: Some(&ingest_job1),
            backoff: taskpg::POD_BACKOFF,
        },
    )
    .await
    .unwrap();
    let archived = archived.expect("the live try archives");
    assert_eq!(archived.state, "failed");
    assert!(archived.live.is_none());
    assert_eq!(archived.archived_reason.as_deref(), Some("retry"));
    let resp = app
        .clone()
        .oneshot(
            bearer(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/internal/attempts/{ingest1}/start")),
                ingest_token,
            )
            .body(Body::from(
                json!({"k8s_job_name": ingest_job1, "pod_uid": "pod-i1"}).to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "archived try is fenced"
    );
    let retry = retry.expect("within budget: a successor");
    assert_eq!(retry.try_number, 2);
    assert_eq!(retry.state, "up_for_retry");
    assert_eq!(retry.id, taskpg::attempt_id(&job.id, "ingest", 2));

    // The §5.5 deadline is DETERMINISTIC: min + hash-jitter, computed from the
    // same transaction timestamp the row's created_at carries.
    let jitter: i64 = client
        .query_one(
            "SELECT mod(abs(hashtext($1)::bigint), $2::bigint)",
            &[&format!("{}#ingest#1", job.id), &30i64],
        )
        .await
        .unwrap()
        .get(0);
    let delay: i64 = client
        .query_one(
            "SELECT floor(extract(epoch FROM (not_before - created_at)))::bigint
             FROM task_instances WHERE id = $1",
            &[&retry.id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        delay,
        30 + jitter,
        "base 30 s plus the deterministic jitter"
    );

    // Try 2: an OOM that never committed escalates M → L (§5.5).
    client
        .execute(
            "UPDATE task_instances SET not_before = now() - interval '1 hour' WHERE id = $1",
            &[&retry.id],
        )
        .await
        .unwrap();
    let ingest_job2 = format!("er-t-{}-l1", &retry.id[..20]);
    taskpg::claim_launch(&client, EPOCH, Some(&ingest_job2), None, 1)
        .await
        .unwrap()
        .expect("try 2 claims");
    let (archived2, retry3) = taskpg::fail_or_retry_task(
        &mut client,
        &retry.id,
        &taskpg::TaskFailure {
            terminal_state: "failed",
            retryable: true,
            escalate: true,
            exit_code: Some(137),
            termination_reason: Some("OOMKilled"),
            error_class: Some("resources"),
            error_detail: Some("container OOMKilled at class M"),
            k8s_job_name: Some(&ingest_job2),
            backoff: taskpg::POD_BACKOFF,
        },
    )
    .await
    .unwrap();
    assert!(archived2.expect("archives").live.is_none());
    let retry3 = retry3.expect("try 2 <= max_tries 2");
    assert_eq!(retry3.try_number, 3);
    assert_eq!(
        retry3.resource_class.as_deref(),
        Some("L"),
        "M escalates to L"
    );
    assert_eq!(retry3.escalated_from.as_deref(), Some("M"));
    assert!(
        retry3.segment.is_none(),
        "a class change invalidates the segment"
    );

    // Try 3 fails terminally: try_number 3 > max_tries 2 — the row stays live
    // in its terminal state and no successor appears.
    client
        .execute(
            "UPDATE task_instances SET not_before = now() - interval '1 hour' WHERE id = $1",
            &[&retry3.id],
        )
        .await
        .unwrap();
    let ingest_job3 = format!("er-t-{}-l1", &retry3.id[..20]);
    taskpg::claim_launch(&client, EPOCH, Some(&ingest_job3), None, 1)
        .await
        .unwrap()
        .expect("try 3 claims");
    let (archived3, retry4) = taskpg::fail_or_retry_task(
        &mut client,
        &retry3.id,
        &taskpg::TaskFailure {
            terminal_state: "failed",
            retryable: true,
            escalate: false,
            exit_code: Some(1),
            termination_reason: None,
            error_class: Some("transient_io"),
            error_detail: Some("still failing"),
            k8s_job_name: Some(&ingest_job3),
            backoff: taskpg::POD_BACKOFF,
        },
    )
    .await
    .unwrap();
    let terminal = archived3.expect("the exhausted try records");
    assert_eq!(
        terminal.live,
        Some(true),
        "the terminal row stays the live row"
    );
    assert_eq!(terminal.state, "failed");
    assert!(retry4.is_none(), "the budget is spent");

    // The lineage walks the retry chain, newest first, and stops at try 1.
    let lineage = er_entities::task_instance::read::retry_lineage(&client, &retry3.id)
        .await
        .unwrap();
    let lineage_ids: Vec<&str> = lineage.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        lineage_ids,
        vec![retry3.id.as_str(), retry.id.as_str(), ingest1.as_str()]
    );

    // ---- Phase D: propagation deadens the chain; the job settles failed ----
    let states = states_for(&client, &dag, &org_id, &job.id).await;
    let actions = er_dag::pass(&dag, &states);
    // standardize, match, reconcile, assemble propagate; finish fires.
    assert_eq!(actions.len(), 5);
    for (idx, action) in &actions[..4] {
        assert_eq!(*action, PassAction::Propagate(TaskState::UpstreamFailed));
        let target = taskpg::attempt_id(&job.id, dag.nodes[*idx].task_id, 1);
        let done =
            er_entities::task_instance::update::propagate(&client, "upstream_failed", &target, 0)
                .await
                .unwrap();
        assert_eq!(done.len(), 1, "{}", dag.nodes[*idx].task_id);
    }
    assert_eq!(
        actions[4],
        (6, PassAction::Schedule),
        "finish fires all_terminal"
    );

    // finish runs (teardown) and the job settles failed from the leaves.
    let finish1 = id_of("finish");
    er_entities::task_instance::update::mark_scheduled(&client, &json!({}), &finish1, 0)
        .await
        .unwrap();
    let finish_job1 = format!("er-t-{}-l1", &finish1[..20]);
    taskpg::claim_launch(&client, EPOCH, Some(&finish_job1), None, 1)
        .await
        .unwrap()
        .expect("the teardown row claims under a running job");
    er_entities::task_instance::update::start(
        &client,
        None,
        "pod-f1",
        None,
        None,
        &finish1,
        &finish_job1,
    )
    .await
    .unwrap();
    er_entities::task_instance::update::finish(
        &client,
        false,
        "done",
        Some(&json!({"plan": "finish_report", "doc": {"settled": "failed"}})),
        None,
        None,
        None,
        Some(0),
        &finish1,
        "pod-f1",
    )
    .await
    .unwrap();

    let states = states_for(&client, &dag, &org_id, &job.id).await;
    assert_eq!(
        settle(&dag, &states),
        SettleVerdict::Settled(TaskState::Failed)
    );
    let settled = er_entities::job::update::settle(
        &client,
        "failed",
        Some("data"),
        Some("task ingest failed after 3 tries"),
        &job.id,
    )
    .await
    .unwrap();
    assert_eq!(settled[0].state, "failed");

    // ---- Phase E: clear — rotation, re-based budgets, reopen, the fence ----
    let seeds = vec![1]; // ingest
    let plan = clear_closure(&dag, &seeds, true, |_| ReferenceExistence::Present);
    let task_ids: Vec<String> = plan
        .must_run
        .iter()
        .map(|&i| dag.nodes[i].task_id.to_string())
        .collect();
    assert_eq!(
        task_ids,
        vec![
            "ingest",
            "standardize",
            "match",
            "reconcile",
            "assemble",
            "finish"
        ]
    );
    let cleared = taskpg::clear(
        &mut client,
        &org_id,
        &job.id,
        &task_ids,
        2,
        None,
        Some("sha256:v2"),
    )
    .await
    .unwrap();
    assert!(cleared.restarting.is_empty(), "nothing was live");
    // ingest try 3 rotates to try 4 with max_tries = 3 + 2; the four
    // propagated rows and finish rotate to try 2 with max_tries = 1 + 2.
    assert_eq!(cleared.rotated.len(), 6);
    let ingest4 = cleared
        .rotated
        .iter()
        .find(|r| r.task_id == "ingest")
        .expect("ingest rotated");
    assert_eq!(ingest4.try_number, 4);
    assert_eq!(ingest4.max_tries, 5);
    assert_eq!(ingest4.state, "none");
    assert_eq!(
        ingest4.resource_class.as_deref(),
        Some("L"),
        "the escalated class carries"
    );
    let finish2 = cleared
        .rotated
        .iter()
        .find(|r| r.task_id == "finish")
        .expect("finish rotated");
    assert_eq!(finish2.max_tries, 3);
    let reopened = cleared.job.expect("the job reopens");
    assert_eq!(reopened.state, "queued");
    assert_eq!(reopened.clear_number, Some(1));
    assert!(reopened.finished_at.is_none());

    // The fence: a verdict composed before the clear (clear_number 0) matches
    // zero rows; the current fence value lands.
    let fenced =
        er_entities::task_instance::update::mark_scheduled(&client, &json!({}), &ingest4.id, 0)
            .await
            .unwrap();
    assert!(fenced.is_empty(), "the stale verdict is fenced");
    // The reopened job re-claims through the existing index and re-plans
    // idempotently.
    let reclaimed = pg::claim(&client, 1)
        .await
        .unwrap()
        .expect("the reopened job claims");
    assert_eq!(reclaimed.id, job.id);
    assert_eq!(reclaimed.attempt, 2, "claim.sql keeps counting claims");
    taskpg::plan(&mut client, &reclaimed, &dag, EPOCH, Some("sha256:v2"))
        .await
        .unwrap();
    let count: i64 = client
        .query_one(
            "SELECT count(*) FROM task_instances WHERE job_id = $1",
            &[&job.id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        count, 15,
        "re-planning inserted nothing (9 archived + 6 live)"
    );

    let scheduled =
        er_entities::task_instance::update::mark_scheduled(&client, &json!({}), &ingest4.id, 1)
            .await
            .unwrap();
    assert_eq!(scheduled.len(), 1, "the post-clear verdict lands");

    // ---- Phase F: clear-while-live → restarting → complete_restart; cancel -
    let ingest4_job = format!("er-t-{}-l1", &ingest4.id[..20]);
    taskpg::claim_launch(&client, EPOCH, Some(&ingest4_job), None, 1)
        .await
        .unwrap()
        .expect("ingest try 4 claims");
    let cleared = taskpg::clear(
        &mut client,
        &org_id,
        &job.id,
        &["ingest".to_string()],
        2,
        Some("M"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(cleared.rotated.len(), 0);
    assert_eq!(
        cleared.restarting.len(),
        1,
        "the live row goes to restarting"
    );
    assert_eq!(cleared.restarting[0].state, "restarting");
    assert_eq!(
        cleared.restarting[0].next_resource_class.as_deref(),
        Some("M")
    );

    // complete_restart once the watch reports the pod gone: rotate with the
    // class override, no try charged as a failure.
    let rotated = er_entities::task_instance::update::archive_restarted(
        &client,
        None,
        Some("revoked"),
        &ingest4.id,
        Some(&ingest4_job),
    )
    .await
    .unwrap();
    assert_eq!(rotated[0].archived_reason.as_deref(), Some("restarted"));
    assert!(
        rotated[0].error_class.is_none(),
        "a restart is evidence, not an error"
    );
    let ingest5 =
        er_entities::task_instance::create::insert_cleared_try(&client, None, 2, None, &ingest4.id)
            .await
            .unwrap();
    assert_eq!(ingest5[0].try_number, 5);
    assert_eq!(
        ingest5[0].resource_class.as_deref(),
        Some("M"),
        "the clear's class override applies to the successor"
    );

    // Cancel: the org slot is held through canceling; the teardown row alone
    // stays claimable (§5.6).
    let std2 = taskpg::attempt_id(&job.id, "standardize", 2);
    // The second clear bumped the fence to 2.
    er_entities::task_instance::update::mark_scheduled(&client, &json!({}), &std2, 2)
        .await
        .unwrap();
    let std2_job = format!("er-t-{}-l1", &std2[..20]);
    taskpg::claim_launch(&client, EPOCH, Some(&std2_job), None, 1)
        .await
        .unwrap()
        .expect("standardize try 2 claims");
    let canceling = er_entities::job::update::cancel(&client, &job.id, &org_id)
        .await
        .unwrap();
    assert_eq!(
        canceling[0].state, "canceling",
        "a running job holds the slot"
    );
    let pending = er_entities::task_instance::update::cancel_pending(&client, &org_id, &job.id)
        .await
        .unwrap();
    assert!(
        pending.iter().all(|r| r.state == "canceled" && !r.teardown),
        "every pending non-teardown row dies directly"
    );
    let live = er_entities::task_instance::update::cancel_live(&client, &org_id, &job.id)
        .await
        .unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].id, std2, "the queued row is marked canceling");
    let settled_cancel = er_entities::task_instance::update::settle_canceled(
        &client,
        None,
        Some("revoked"),
        &std2,
        Some(&std2_job),
    )
    .await
    .unwrap();
    assert_eq!(settled_cancel[0].state, "canceled");

    // The teardown finish re-arms under canceling: rotated to try 2 by the
    // clear, it schedules and claims while the job is canceling.
    let finish2_id = taskpg::attempt_id(&job.id, "finish", 2);
    er_entities::task_instance::update::mark_scheduled(&client, &json!({}), &finish2_id, 2)
        .await
        .unwrap();
    let finish2_job = format!("er-t-{}-l1", &finish2_id[..20]);
    let td = taskpg::claim_launch(&client, EPOCH, Some(&finish2_job), None, 1)
        .await
        .unwrap()
        .expect("the teardown row is claimable under canceling");
    assert_eq!(td.id, finish2_id);
    er_entities::task_instance::update::start(
        &client,
        None,
        "pod-f2",
        None,
        None,
        &finish2_id,
        &finish2_job,
    )
    .await
    .unwrap();
    er_entities::task_instance::update::finish(
        &client,
        false,
        "done",
        None,
        None,
        None,
        None,
        Some(0),
        &finish2_id,
        "pod-f2",
    )
    .await
    .unwrap();
    let job_settled = er_entities::job::update::settle(&client, "canceled", None, None, &job.id)
        .await
        .unwrap();
    assert_eq!(job_settled[0].state, "canceled");

    // ---- Phase G: the control claim (§3.6) --------------------------------
    let train_dag = dags::dag_for(JobKind::Train);
    pg::enqueue(
        &client,
        &org_id,
        None,
        "train",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    let train_job = pg::claim(&client, 1)
        .await
        .unwrap()
        .expect("the slot freed at cancel");
    taskpg::plan(&mut client, &train_job, &train_dag, EPOCH, None)
        .await
        .unwrap();
    let register1 = taskpg::attempt_id(&train_job.id, "register_model", 1);
    er_entities::task_instance::update::mark_scheduled(&client, &json!({}), &register1, 0)
        .await
        .unwrap();
    let control = er_entities::task_instance::update::claim_control(&client, EPOCH)
        .await
        .unwrap();
    assert_eq!(control.len(), 1);
    assert_eq!(control[0].id, register1);
    assert_eq!(control[0].state, "running");
    assert!(
        control[0].resource_class.is_none(),
        "a control row has no class"
    );
    // A control row holds no org slot: a pod row of the same org still claims.
    let schema1 = taskpg::attempt_id(&train_job.id, "reconcile_schema", 1);
    er_entities::task_instance::update::mark_scheduled(&client, &json!({}), &schema1, 0)
        .await
        .unwrap();
    let schema_job = format!("er-t-{}-l1", &schema1[..20]);
    taskpg::claim_launch(&client, EPOCH, Some(&schema_job), None, 2)
        .await
        .unwrap()
        .expect("a control row does not hold the pod slot");
    // finish_control is fenced by the dispatcher epoch.
    let foreign = er_entities::task_instance::update::finish_control(
        &client,
        false,
        "done",
        None,
        None,
        &register1,
        "epoch-other",
    )
    .await
    .unwrap();
    assert!(foreign.is_empty(), "a previous epoch's finish is a no-op");
    let done = er_entities::task_instance::update::finish_control(
        &client,
        false,
        "done",
        Some(&json!({"plan": "model_registered", "doc": {"label": "trained-x"}})),
        None,
        &register1,
        EPOCH,
    )
    .await
    .unwrap();
    assert_eq!(done[0].state, "succeeded");

    // ---- Phase H: the fused segment's finish-and-advance (§3.7) -----------
    // reconcile_schema (claimed above) and a synthetic same-class segment
    // sibling: the handler moves k+1 into the pod's identity in the same
    // transaction as k's finish, so the one-pod index holds at every boundary.
    let train1 = taskpg::attempt_id(&train_job.id, "train", 1);
    client
        .execute(
            "UPDATE task_instances SET segment = 's1', resource_class = 'M' WHERE id = ANY($1)",
            &[&vec![schema1.clone(), train1.clone()]],
        )
        .await
        .unwrap();
    er_entities::task_instance::update::start(
        &client,
        None,
        "pod-seg",
        None,
        None,
        &schema1,
        &schema_job,
    )
    .await
    .unwrap();
    let advance = taskpg::AdvanceArgs {
        next_task_instance_id: &train1,
        job_id: &train_job.id,
        segment: "s1",
        resource_class: "M",
        clear_number: 0,
        input: &json!({"pins": ["schema_reconciled"]}),
    };
    let (finished, advanced) = taskpg::finish_and_advance(
        &mut client,
        &schema1,
        "pod-seg",
        false,
        "done",
        Some(&json!({"plan": "schema_reconciled", "doc": {"status": "ok", "catalog_hash": "h"}})),
        (None, None),
        None,
        Some(0),
        Some(&advance),
    )
    .await
    .unwrap();
    assert_eq!(finished.expect("k finishes").state, "succeeded");
    let advanced = advanced.expect("k+1 advances inside the same transaction");
    assert_eq!(advanced.id, train1);
    assert_eq!(advanced.state, "running");
    assert_eq!(advanced.k8s_job_name.as_deref(), Some(schema_job.as_str()));
    assert_eq!(advanced.pod_uid.as_deref(), Some("pod-seg"));
    assert_eq!(advanced.launches, 1);
    // Exactly one live pod row sits in the one-pod predicate after the
    // hand-off.
    let in_slot: i64 = client
        .query_one(
            "SELECT count(*) FROM task_instances WHERE org_id = $1 AND live IS TRUE
             AND kind = 'pod' AND state IN ('queued','running','restarting','canceling')",
            &[&org_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(in_slot, 1);
}
