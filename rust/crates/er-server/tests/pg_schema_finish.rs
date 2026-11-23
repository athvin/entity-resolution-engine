//! Schema-position publication is atomic with the task receipt that reports it.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use tower::ServiceExt;

use er_server::dags::{self, JobKind};
use er_server::{AppState, pg, taskpg};

use crate::common;

const EPOCH: &str = "epoch-1";

fn bearer(req: axum::http::request::Builder, token: &str) -> axum::http::request::Builder {
    req.header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
}

/// Drive one lake_migrate job's single `reconcile_schema` task to its started
/// line over the attempt channel, returning the attempt id and its token.
async fn started_schema_attempt(
    client: &mut tokio_postgres::Client,
    app: &axum::Router,
    org_id: &str,
    token: &str,
    pod_uid: &str,
) -> (String, String) {
    let job = pg::enqueue(
        client,
        org_id,
        None,
        "lake_migrate",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    let claimed = pg::claim(client, 1).await.unwrap().expect("claimable");
    let dag = dags::dag_for(JobKind::LakeMigrate);
    taskpg::plan(client, &claimed, &dag, EPOCH, Some("sha256:test"))
        .await
        .unwrap()
        .expect("plannable");
    let attempt = taskpg::attempt_id(&job.id, "reconcile_schema", 1);
    let scheduled = er_entities::task_instance::update::mark_scheduled(
        client,
        &json!({"pins": [], "params": {}}),
        &attempt,
        0,
    )
    .await
    .unwrap();
    assert_eq!(scheduled.len(), 1);
    let job_name = format!("er-t-{}-l1", &attempt[..20]);
    let launched = taskpg::claim_launch(
        client,
        EPOCH,
        Some(&job_name),
        Some(&pg::hash_secret(token)),
        1,
    )
    .await
    .unwrap()
    .expect("the scheduled reconcile_schema is claimable");
    assert_eq!(launched.id, attempt);
    let resp = app
        .clone()
        .oneshot(
            bearer(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/internal/attempts/{attempt}/start")),
                token,
            )
            .body(Body::from(
                json!({"k8s_job_name": job_name, "pod_uid": pod_uid, "pod_name": "er-t-rec", "node_name": "node-1"})
                    .to_string(),
            ))
            .unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    (job.id, attempt)
}

/// The finish handler carries a schema task's tenant position in the SAME
/// transaction as the settlement, so dependent reporting can never observe a
/// settled task whose position was not recorded — the detached-write race that
/// briefly told the deploy sweep a breaking tenant was `current` (#1155, seen
/// in run 44877542413). No sleeps: a 200 from the finish IS the durability
/// boundary, which is the only place a deterministic test can draw it.
#[tokio::test]
async fn a_schema_task_finish_records_its_tenant_position_with_the_settlement() {
    let Some(db) = common::fresh_db("schema_finish").await else {
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
    let app = er_server::app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    // ---- Phase 1: a FAILED reconcile records `breaking` with the settlement -
    let (_job, attempt) =
        started_schema_attempt(&mut client, &app, &org_id, "tok-rec-1", "pod-rec-1").await;
    let detail = "ERR_SCHEMA_BREAKING runs.snapshot_start: DOUBLE -> BIGINT";
    let resp = app
        .clone()
        .oneshot(
            bearer(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/internal/attempts/{attempt}/finish")),
                "tok-rec-1",
            )
            .body(Body::from(
                json!({"state": "failed", "pod_uid": "pod-rec-1", "error_class": "data",
                       "error_detail": detail, "exit_code": 1})
                .to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // The 200 already contains the position: read it back with no settle wait.
    let states = er_entities::tenant_schema_state::read::list(&client)
        .await
        .unwrap();
    assert_eq!(states.len(), 1, "one recorded position: {states:?}");
    assert_eq!(states[0].org_id, org_id);
    assert_eq!(states[0].status, "breaking");
    assert!(
        states[0]
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains(detail),
        "{states:?}"
    );

    // ---- Phase 2: a stale replay of that finish is refused and rewrites nothing
    let resp = app
        .clone()
        .oneshot(
            bearer(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/internal/attempts/{attempt}/finish")),
                "tok-rec-1",
            )
            .body(Body::from(
                json!({"state": "failed", "pod_uid": "pod-rec-1", "error_class": "data",
                       "error_detail": "ERR_SCHEMA_BREAKING a different, later report",
                       "exit_code": 1})
                .to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let after = er_entities::tenant_schema_state::read::list(&client)
        .await
        .unwrap();
    assert_eq!(after.len(), 1);
    assert!(
        after[0]
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains(detail),
        "a stale finish overwrote the recorded position: {after:?}"
    );

    // ---- Phase 3: a SUCCEEDED reconcile's receipt lands the same way --------
    // The failed job remains active until its DAG's finish node settles, so a
    // second org avoids the intentional one-active-job-per-org admission rule.
    pg::ensure_org(&client, "acme-success", "/tmp/acme-success.yaml")
        .await
        .unwrap();
    let success_org_id: String = client
        .query_one("SELECT id FROM orgs WHERE name = 'acme-success'", &[])
        .await
        .unwrap()
        .get(0);
    let (_job, attempt) =
        started_schema_attempt(&mut client, &app, &success_org_id, "tok-rec-2", "pod-rec-2").await;
    let resp = app
        .clone()
        .oneshot(
            bearer(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/internal/attempts/{attempt}/finish")),
                "tok-rec-2",
            )
            .body(Body::from(
                json!({"state": "succeeded", "pod_uid": "pod-rec-2", "outcome": "done",
                       "output": {"plan": "schema_reconciled",
                                  "doc": {"status": "current", "catalog_hash": "abc123",
                                          "detail": "nothing pending"}},
                       "exit_code": 0})
                .to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let states = er_entities::tenant_schema_state::read::list(&client)
        .await
        .unwrap();
    assert_eq!(states.len(), 2);
    let success = states
        .iter()
        .find(|state| state.org_id == success_org_id)
        .expect("the successful org's position");
    assert_eq!(success.status, "current");
    assert_eq!(success.catalog_hash, "abc123");

    // ---- Phase 4: a position-write failure rolls back the receipt -----------
    pg::ensure_org(&client, "acme-retry", "/tmp/acme-retry.yaml")
        .await
        .unwrap();
    let retry_org_id: String = client
        .query_one("SELECT id FROM orgs WHERE name = 'acme-retry'", &[])
        .await
        .unwrap()
        .get(0);
    let (job, attempt) =
        started_schema_attempt(&mut client, &app, &retry_org_id, "tok-rec-3", "pod-rec-3").await;
    client
        .batch_execute(
            "CREATE FUNCTION reject_tenant_position() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN RAISE EXCEPTION 'injected tenant position failure'; END;
             $$;
             CREATE TRIGGER reject_tenant_position BEFORE INSERT OR UPDATE
             ON tenant_schema_states FOR EACH ROW EXECUTE FUNCTION reject_tenant_position();",
        )
        .await
        .unwrap();
    let finish = || {
        bearer(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/internal/attempts/{attempt}/finish")),
            "tok-rec-3",
        )
        .body(Body::from(
            json!({"state": "succeeded", "pod_uid": "pod-rec-3", "outcome": "done",
                   "output": {"plan": "schema_reconciled",
                              "doc": {"status": "current", "catalog_hash": "retry123",
                                      "detail": "nothing pending"}},
                   "exit_code": 0})
            .to_string(),
        ))
        .unwrap()
    };
    let resp = app.clone().oneshot(finish()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let tasks = er_entities::task_instance::read::by_job(&client, &retry_org_id, &job)
        .await
        .unwrap();
    let unsettled = tasks.iter().find(|task| task.id == attempt).unwrap();
    assert_eq!(unsettled.state, "running");
    assert_eq!(unsettled.live, Some(true));
    assert_eq!(
        er_entities::tenant_schema_state::read::list(&client)
            .await
            .unwrap()
            .len(),
        2,
        "a failed position write published neither a new position nor a settlement"
    );

    client
        .batch_execute(
            "DROP TRIGGER reject_tenant_position ON tenant_schema_states;
             DROP FUNCTION reject_tenant_position();",
        )
        .await
        .unwrap();
    let resp = app.clone().oneshot(finish()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let tasks = er_entities::task_instance::read::by_job(&client, &retry_org_id, &job)
        .await
        .unwrap();
    assert_eq!(
        tasks.iter().find(|task| task.id == attempt).unwrap().state,
        "succeeded"
    );
    let states = er_entities::tenant_schema_state::read::list(&client)
        .await
        .unwrap();
    let retried = states
        .iter()
        .find(|state| state.org_id == retry_org_id)
        .expect("the retry publishes its position");
    assert_eq!(retried.status, "current");
    assert_eq!(retried.catalog_hash, "retry123");
}
