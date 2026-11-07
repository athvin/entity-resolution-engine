//! The Kubernetes executor end to end through the child-process double (§6.9,
//! gated on BOTH `ER_TEST_PG_DSN` and `DEDUPE_E2E_DUCKLAKE=1`): a real
//! `lake_maintain` job is claimed and PLANNED by the executor, its two tasks
//! launch as "pods" that run the real `er-server exec-task` binary against a
//! real er-api over the attempt channel, the watch frames settle the rows, and
//! the job settles `succeeded` — then a scripted OOMKill proves the §6.5
//! resources path retries on a fresh try before the real body recovers.
//!
//! One `#[tokio::test]` with sequential phases (`pg_serial.rs`).

use std::sync::Arc;

use er_server::executor::{Executor, ExecutorConfig};
use er_server::{AppState, dispatcher, pg};

use crate::common;
use crate::executor_harness::pump;
use crate::fake_pod_api::{FakePodApi, Script};

const TEST_YAML: &str = include_str!("../../er-cli/tests/fixtures/test.yaml");

async fn job_state(client: &tokio_postgres::Client, org: &str, id: &str) -> String {
    pg::get_job(client, org, id)
        .await
        .unwrap()
        .map(|j| j.state)
        .unwrap_or_default()
}

#[tokio::test]
async fn lake_maintain_runs_end_to_end_through_the_executor() {
    let Some(db) = common::fresh_db("run_all_dag").await else {
        return;
    };
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping the executor e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    // A tenant with a real (empty) lake: maintain is meaningful over any
    // initialized lake.
    let tmp = std::env::temp_dir().join(format!("er_dag_exec_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let config_path = tmp.join("acme.yaml");
    std::fs::write(&config_path, TEST_YAML).unwrap();
    let env = serde_json::json!({
        "ER_LAKE_CATALOG": tmp.join("catalog.ducklake").to_string_lossy(),
        "ER_LAKE_DATA_PATH": tmp.join("data").to_string_lossy(),
        "ER_CONFIG_PATH": config_path.to_string_lossy(),
    });
    pg::upsert_org_byo(
        &client,
        "acme",
        config_path.to_str().unwrap(),
        &env,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let org_id = pg::org_id_by_name(&client, "acme")
        .await
        .unwrap()
        .expect("org");

    // The REAL control plane, on a local port: the child processes report over
    // the same attempt channel a pod would.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = er_server::app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let (api, mut frames) = FakePodApi::new(format!("http://{addr}"), tmp.clone());
    let api = Arc::new(api);
    let cfg = ExecutorConfig {
        // Neither kind here provisions, so no profile is needed.
        profile: Ok(None),
        in_flight_cap: 2,
        template: er_k8s::manifest::REFERENCE_TEMPLATE.to_string(),
        image_digest: Some("sha256:test".into()),
    };
    let mut executor = Executor::connect(&dsn, Arc::clone(&api) as Arc<dyn er_k8s::PodApi>, cfg)
        .await
        .unwrap();
    executor.adopt().await;

    // ---- phase 1: the green path -------------------------------------------
    let queued = pg::enqueue(
        &client,
        &org_id,
        None,
        "lake_maintain",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    let ran = dispatcher::dispatch_once(&client, Some(&mut executor))
        .await
        .unwrap();
    assert!(ran, "the executor claimed and planned the job");
    assert_eq!(job_state(&client, &org_id, &queued.id).await, "running");
    let rows = er_entities::task_instance::read::by_job(&client, &org_id, &queued.id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2, "reconcile_schema and maintain");

    // Drive the machine until the job settles (child processes run for real).
    for _ in 0..300 {
        pump(&mut executor, &mut frames).await;
        if job_state(&client, &org_id, &queued.id).await != "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert_eq!(job_state(&client, &org_id, &queued.id).await, "succeeded");
    let rows = er_entities::task_instance::read::by_job(&client, &org_id, &queued.id)
        .await
        .unwrap();
    for row in &rows {
        assert_eq!(row.state, "succeeded", "task {}", row.task_id);
        assert!(
            row.k8s_job_name
                .as_deref()
                .unwrap_or_default()
                .starts_with("er-t-"),
            "a real derived Job name, not inproc: {:?}",
            row.k8s_job_name
        );
        assert!(
            row.pod_uid
                .as_deref()
                .unwrap_or_default()
                .starts_with("fake-uid-")
        );
        assert!(
            row.token_hash.is_none(),
            "the fallback token is nulled at finish"
        );
        assert!(row.output.is_some(), "the durable output landed");
    }
    let schema_row = rows
        .iter()
        .find(|r| r.task_id == "reconcile_schema")
        .unwrap();
    assert_eq!(
        schema_row.output.as_ref().unwrap()["plan"],
        "schema_reconciled"
    );
    // The §6.4 relay: the pod carries no control-plane credential, yet the
    // tenant's schema state was recorded — through the finish handler.
    let recorded: i64 = client
        .query_one(
            "SELECT count(*) FROM tenant_schema_states WHERE org_id = $1",
            &[&org_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(recorded, 1, "tenant_schema_states gained acme's row");

    // ---- phase 2: a scripted OOM retries on a fresh try, then recovers ------
    let second = pg::enqueue(
        &client,
        &org_id,
        None,
        "lake_maintain",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    assert!(
        dispatcher::dispatch_once(&client, Some(&mut executor))
            .await
            .unwrap()
    );
    let rows = er_entities::task_instance::read::by_job(&client, &org_id, &second.id)
        .await
        .unwrap();
    let maintain1 = rows
        .iter()
        .find(|r| r.task_id == "maintain")
        .expect("the maintain row")
        .id
        .clone();
    api.script(&maintain1, Script::OomKill);

    // Drive until the OOM try archives and its successor appears.
    let mut retry = None;
    for _ in 0..300 {
        pump(&mut executor, &mut frames).await;
        let rows = er_entities::task_instance::read::by_job(&client, &org_id, &second.id)
            .await
            .unwrap();
        if let Some(r) = rows
            .iter()
            .find(|r| r.task_id == "maintain" && r.try_number == 2)
        {
            retry = Some(r.clone());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let retry = retry.expect("an OOMKilled try earns a successor (§5.5)");
    assert_eq!(retry.state, "up_for_retry");
    let rows = er_entities::task_instance::read::by_job(&client, &org_id, &second.id)
        .await
        .unwrap();
    let archived = rows
        .iter()
        .find(|r| r.task_id == "maintain" && r.try_number == 1)
        .unwrap();
    assert!(archived.live.is_none());
    assert_eq!(archived.error_class.as_deref(), Some("resources"));
    assert_eq!(archived.termination_reason.as_deref(), Some("OOMKilled"));

    // Recovery: unscript, clear the backoff, and let the real body run try 2.
    api.unscript(&maintain1);
    client
        .execute(
            "UPDATE task_instances SET not_before = now() WHERE id = $1",
            &[&retry.id],
        )
        .await
        .unwrap();
    for _ in 0..300 {
        pump(&mut executor, &mut frames).await;
        if job_state(&client, &org_id, &second.id).await != "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert_eq!(
        job_state(&client, &org_id, &second.id).await,
        "succeeded",
        "try 2 ran the real body and the job settled"
    );

    // ---- phase 3: cancel through canceling with the sweep -------------------
    let third = pg::enqueue(
        &client,
        &org_id,
        None,
        "lake_maintain",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    assert!(
        dispatcher::dispatch_once(&client, Some(&mut executor))
            .await
            .unwrap()
    );
    // Cancel while running: the org slot stays held until the rows settle.
    let canceled = er_entities::job::update::cancel(&client, &third.id, &org_id)
        .await
        .unwrap();
    assert_eq!(canceled[0].state, "canceling");
    for _ in 0..300 {
        pump(&mut executor, &mut frames).await;
        if job_state(&client, &org_id, &third.id).await == "canceled" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert_eq!(job_state(&client, &org_id, &third.id).await, "canceled");
    let rows = er_entities::task_instance::read::by_job(&client, &org_id, &third.id)
        .await
        .unwrap();
    assert!(
        rows.iter()
            .filter(|r| r.live == Some(true))
            .all(|r| matches!(r.state.as_str(), "canceled" | "succeeded")),
        "every live row settled under the cancel: {:?}",
        rows.iter()
            .map(|r| (&r.task_id, &r.state))
            .collect::<Vec<_>>()
    );
    // The slot is free again: a fourth job claims.
    let fourth = pg::enqueue(
        &client,
        &org_id,
        None,
        "lake_maintain",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    assert!(
        dispatcher::dispatch_once(&client, Some(&mut executor))
            .await
            .unwrap()
    );
    assert_eq!(job_state(&client, &org_id, &fourth.id).await, "running");
    // Settle it so the database ends quiet.
    for _ in 0..300 {
        pump(&mut executor, &mut frames).await;
        if job_state(&client, &org_id, &fourth.id).await != "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert_eq!(job_state(&client, &org_id, &fourth.id).await, "succeeded");

    let _ = std::fs::remove_dir_all(&tmp);
}
