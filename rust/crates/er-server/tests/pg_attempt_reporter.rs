//! The attempt reporter end to end (gated on BOTH `ER_TEST_PG_DSN` and
//! `DEDUPE_E2E_DUCKLAKE=1`): the REAL dispatcher claims real jobs and the
//! ledger fills with real stages — a successful `run_all_full` leaves seven
//! `succeeded` rows under the synthetic `inproc` identity; an engine failure
//! lands on the stage that failed and deadens its chain; a transient job retry
//! clears and replans the rows with the fence advanced (§11 Phase 1).
//!
//! One `#[tokio::test]` with sequential phases (`pg_serial.rs`).

use std::path::PathBuf;

use er_server::reporter::AttemptReporter;
use er_server::{dispatcher, pg};

use crate::common;

const TEST_YAML: &str = include_str!("../../er-cli/tests/fixtures/test.yaml");

fn repo_fixture(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/static")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

async fn oid(client: &tokio_postgres::Client, name: &str) -> String {
    pg::org_id_by_name(client, name)
        .await
        .expect("org lookup")
        .expect("org exists")
}

async fn task_rows(
    client: &tokio_postgres::Client,
    org_id: &str,
    job_id: &str,
) -> Vec<er_entities::task_instance::TaskInstance> {
    er_entities::task_instance::read::by_job(client, org_id, job_id)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_reporter_fills_the_ledger_from_the_legacy_dispatcher() {
    let Some(db) = common::fresh_db("attempt_reporter").await else {
        return;
    };
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping the reporter e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    // acme: a complete, runnable tenant (lake + drop + config + frozen model).
    let tmp = std::env::temp_dir().join(format!("er_reporter_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    std::fs::create_dir_all(tmp.join("drop")).unwrap();
    let config_path = tmp.join("acme.yaml");
    std::fs::write(&config_path, TEST_YAML).unwrap();
    for source in ["crm", "billing", "webforms"] {
        std::fs::copy(
            repo_fixture(&format!("base_10/base/{source}.csv")),
            tmp.join("drop").join(format!("{source}.csv")),
        )
        .unwrap();
    }
    let env = serde_json::json!({
        "ER_LAKE_CATALOG": tmp.join("catalog.ducklake").to_string_lossy(),
        "ER_LAKE_DATA_PATH": tmp.join("data").to_string_lossy(),
        "ER_CONFIG_PATH": config_path.to_string_lossy(),
        "ER_MODEL_PATH": repo_fixture("model_test_v1.json"),
        "ER_TF_PATH": repo_fixture("model_test_v1.tf.csv"),
        "ER_TF_SNAPSHOT_ID": "01JWMDTFSNAP00000000000001",
    });
    pg::upsert_org_byo(
        &client,
        "acme",
        config_path.to_str().unwrap(),
        &env,
        Some(&tmp.join("drop").to_string_lossy()),
        None,
        None,
    )
    .await
    .unwrap();
    let acme = oid(&client, "acme").await;

    // empty: a valid env over an empty lake, for the engine-failure phase.
    std::fs::create_dir_all(tmp.join("empty/data")).unwrap();
    let empty_env = serde_json::json!({
        "ER_LAKE_CATALOG": tmp.join("empty/catalog.ducklake").to_string_lossy(),
        "ER_LAKE_DATA_PATH": tmp.join("empty/data").to_string_lossy(),
        "ER_CONFIG_PATH": config_path.to_string_lossy(),
    });
    pg::upsert_org_byo(
        &client,
        "empty",
        config_path.to_str().unwrap(),
        &empty_env,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let empty = oid(&client, "empty").await;

    // brokenlake: an unopenable catalog path, for the transient-retry phase.
    let broken_env = serde_json::json!({
        "ER_LAKE_CATALOG": tmp.join("nowhere/at/all/catalog.ducklake").to_string_lossy(),
        "ER_LAKE_DATA_PATH": tmp.join("nowhere/at/all/data").to_string_lossy(),
        "ER_CONFIG_PATH": config_path.to_string_lossy(),
    });
    pg::upsert_org_byo(
        &client,
        "brokenlake",
        config_path.to_str().unwrap(),
        &broken_env,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let broken = oid(&client, "brokenlake").await;

    let mut reporter = AttemptReporter::connect(&dsn).await.unwrap();
    let mailer = er_server::mail::Mailer::off();

    // ---- phase 1: a green run_all fills seven succeeded rows ---------------
    let queued = pg::enqueue(&client, &acme, "run_all_full", 0, None, None, None)
        .await
        .unwrap();
    let ran = dispatcher::dispatch_once_with(&client, &Ok(None), &mailer, Some(&mut reporter))
        .await
        .unwrap();
    assert!(ran, "the queued run_all was claimed");
    let job = pg::get_job(&client, &acme, &queued.id)
        .await
        .unwrap()
        .expect("the job row");
    assert_eq!(job.state, "succeeded");
    assert_eq!(
        job.run_id.as_deref(),
        Some(queued.id.as_str()),
        "planning finally writes run_id = id (§4.1)"
    );
    assert!(
        job.structural_fingerprint.is_some() && job.policy_hash.is_some(),
        "planning stamps both hashes"
    );
    let rows = task_rows(&client, &acme, &queued.id).await;
    assert_eq!(rows.len(), 7, "one row per run_all node");
    for row in &rows {
        assert_eq!(row.state, "succeeded", "task {}", row.task_id);
        assert_eq!(row.live, Some(true));
        assert_eq!(row.try_number, 1);
        assert_eq!(
            row.k8s_job_name.as_deref(),
            Some("inproc"),
            "the synthetic identity names the in-process path"
        );
        assert!(
            row.snapshot_end.is_some(),
            "task {} records the lake head it committed",
            row.task_id
        );
        assert!(row.started_at.is_some() && row.finished_at.is_some());
    }
    // Stage order is execution order: seq ascending matches the started_at
    // order for a sequential in-process run.
    let mut by_seq = rows.clone();
    by_seq.sort_by_key(|r| r.seq);
    let starts: Vec<_> = by_seq.iter().map(|r| r.started_at.unwrap()).collect();
    assert!(
        starts.windows(2).all(|w| w[0] <= w[1]),
        "stages started in topological order"
    );

    // ---- phase 2: an engine failure lands on its stage and deadens the chain
    let correct_job = pg::enqueue(&client, &empty, "correct", 0, None, None, None)
        .await
        .unwrap();
    dispatcher::dispatch_once_with(&client, &Ok(None), &mailer, Some(&mut reporter))
        .await
        .unwrap();
    let failed = pg::get_job(&client, &empty, &correct_job.id)
        .await
        .unwrap()
        .expect("the job row");
    assert_eq!(failed.state, "failed", "no prior run to correct");
    let rows = task_rows(&client, &empty, &correct_job.id).await;
    assert_eq!(rows.len(), 4, "init, reconcile, assemble, finish");
    let state_of = |task: &str| {
        rows.iter()
            .find(|r| r.task_id == task && r.live == Some(true))
            .map(|r| r.state.clone())
            .expect(task)
    };
    assert_eq!(state_of("init"), "failed");
    let init = rows.iter().find(|r| r.task_id == "init").unwrap();
    assert!(
        init.error_detail
            .as_deref()
            .is_some_and(|d| d.contains("no prior run to correct")),
        "the stage failure carries the engine's own text: {:?}",
        init.error_detail
    );
    assert_eq!(init.error_class.as_deref(), Some("precondition"));
    assert_eq!(state_of("reconcile"), "upstream_failed");
    assert_eq!(state_of("assemble"), "upstream_failed");
    assert_eq!(
        state_of("finish"),
        "canceled",
        "the teardown's rule would fire, but the legacy arm has no teardown execution"
    );

    // ---- phase 3: a transient job retry clears and replans, fence advanced -
    let broken_job = pg::enqueue(&client, &broken, "run_all_full", 0, None, None, None)
        .await
        .unwrap();
    // Attempt 1: the lake cannot open — a transient class, so the JOB requeues
    // and the rows are left for the next claim's clear to rotate.
    dispatcher::dispatch_once_with(&client, &Ok(None), &mailer, Some(&mut reporter))
        .await
        .unwrap();
    let after_one = pg::get_job(&client, &broken, &broken_job.id)
        .await
        .unwrap()
        .expect("the job row");
    assert_eq!(after_one.state, "queued", "open-lake failures retry");
    assert_eq!(after_one.attempt, 1);
    let rows = task_rows(&client, &broken, &broken_job.id).await;
    assert_eq!(rows.len(), 7);
    // The pre-step lake reconcile is the first thing that touches the broken
    // lake, and for a run_all kind that failure lands on `init` — the task
    // whose first duty the reconcile is (§3.5).
    let init = rows
        .iter()
        .find(|r| r.task_id == "init" && r.live == Some(true))
        .expect("init's live row");
    assert_eq!(init.state, "failed");
    assert_eq!(init.error_class.as_deref(), Some("transient_io"));
    assert!(
        rows.iter()
            .filter(|r| r.task_id != "init")
            .all(|r| r.state == "none"),
        "nothing past init moved"
    );

    // Attempts 2 and 3: drive to the terminal failure.
    for _ in 0..2 {
        client
            .execute(
                "UPDATE jobs SET not_before = now() - interval '1 hour' WHERE id = $1",
                &[&broken_job.id],
            )
            .await
            .unwrap();
        dispatcher::dispatch_once_with(&client, &Ok(None), &mailer, Some(&mut reporter))
            .await
            .unwrap();
    }
    let terminal = pg::get_job(&client, &broken, &broken_job.id)
        .await
        .unwrap()
        .expect("the job row");
    assert_eq!(terminal.state, "failed", "the attempt budget is spent");
    assert_eq!(
        terminal.clear_number,
        Some(2),
        "each retry's replan cleared the previous image (the fence advanced twice)"
    );
    let rows = task_rows(&client, &broken, &broken_job.id).await;
    // Each retry's clear rotated init's terminal row (tries 1..=3 stay as
    // history) and reset the six pending rows in place.
    assert_eq!(rows.len(), 9, "7 live + init's two archived tries");
    let live: Vec<_> = rows.iter().filter(|r| r.live == Some(true)).collect();
    assert_eq!(live.len(), 7);
    let init = live.iter().find(|r| r.task_id == "init").expect("init");
    assert_eq!(init.state, "failed");
    assert_eq!(
        init.try_number, 3,
        "one try per job attempt, rotated by the clears"
    );
    for row in live.iter().filter(|r| r.task_id != "init") {
        let expected = if row.task_id == "finish" {
            // The teardown's rule would fire; the legacy arm has no teardown
            // execution, so it records canceled.
            "canceled"
        } else {
            "upstream_failed"
        };
        assert_eq!(
            row.state, expected,
            "task {} settles under the advanced fence",
            row.task_id
        );
    }
    assert_eq!(
        rows.iter()
            .filter(|r| r.live.is_none() && r.archived_reason.as_deref() == Some("cleared"))
            .count(),
        2,
        "the clears archived init's spent tries as history"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
