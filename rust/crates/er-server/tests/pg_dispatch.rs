//! The REAL dispatcher end to end (gated on BOTH `ER_TEST_PG_DSN` and
//! `DEDUPE_E2E_DUCKLAKE=1`): a job claimed from the live Postgres SKIP-LOCKED
//! queue drives `er_cli::run_all` over a per-org DuckLake, and the outcome is
//! written back to the queue — the control -> data-plane bridge. Also proves the
//! failure path (runner error -> `failed`) and the empty-queue path (`None`).
//! Run with e.g.
//!   ER_TEST_PG_DSN='...' DEDUPE_E2E_DUCKLAKE=1 \
//!     cargo test -p er-server --test pg_dispatch

use std::path::PathBuf;

use er_server::{dispatch_pg, pg};

/// The org's id for a test org name (child wrappers key on org_id).
async fn oid(client: &tokio_postgres::Client, name: &str) -> String {
    er_server::pg::org_id_by_name(client, name)
        .await
        .expect("org lookup")
        .expect("org exists")
}

const TEST_YAML: &str = include_str!("../../er-cli/tests/fixtures/test.yaml");
const NICKNAMES: &str = include_str!("../../er-cli/tests/fixtures/nickname_variants.csv");

/// A repo-root `fixtures/static/<rel>` path (same layout er-cli's lake_e2e uses).
fn repo_fixture(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/static")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

#[tokio::test]
async fn dispatcher_runs_run_all_over_a_per_org_ducklake() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping dispatcher e2e (set ER_TEST_PG_DSN to run)");
        return;
    };
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping dispatcher e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    // Control plane: a clean queue with one org and one queued job.
    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS staged_steward_actions, audit_log, schedules, config_versions, api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::ensure_org(&client, "acme", "/configs/acme.yaml")
        .await
        .unwrap();
    let queued = pg::enqueue(
        &client,
        &oid(&client, "acme").await,
        "run_all_full",
        0,
        None,
        None,
    )
    .await
    .unwrap();

    // Data plane: a per-org DuckLake + base_10 inputs (config, lexicon,
    // deliveries, frozen model) — the inputs the production dispatcher resolves
    // from the org's config + drop files + model registry.
    let tmp = std::env::temp_dir().join(format!("er_dispatch_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();
    let session = er_cli::open_lake(&catalog, &data_path).unwrap();
    let config = er_config::load_config_str(TEST_YAML).unwrap();

    let lexicon: Vec<(String, String)> = NICKNAMES
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (a, b) = l.split_once(',').unwrap();
            (a.to_string(), b.to_string())
        })
        .collect();
    let deliveries: Vec<(String, String)> = ["crm", "billing", "webforms"]
        .iter()
        .map(|s| {
            (
                (*s).to_string(),
                std::fs::read_to_string(repo_fixture(&format!("base_10/base/{s}.csv"))).unwrap(),
            )
        })
        .collect();
    let model_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let tf_csv = repo_fixture("model_test_v1.tf.csv");
    let model = er_cli::MatchModel {
        settings_json: &model_json,
        tf_csv_path: std::path::Path::new(&tf_csv),
        model_version: "v0001",
        tf_snapshot_id: "01JWMDTFSNAP00000000000001",
    };
    let inputs = er_cli::PipelineInputs {
        lexicon: &lexicon,
        deliveries: &deliveries,
        model: &model,
        run_id: "01DISPATCHRUN00000000000000",
        ingested_at: "2026-01-01 00:00:00",
        scored_at: "2026-01-02 00:00:00",
        run_at: "2026-01-03 00:00:00",
    };

    // Dispatch: claim the job, run the whole pipeline, mark the outcome.
    let done = dispatch_pg(
        &client,
        |job| {
            assert_eq!(job.id, queued.id, "claimed the queued job");
            assert_eq!(job.state, "dispatching", "claim moved it to dispatching");
            er_cli::run_all(&session, &config, &inputs).map_err(|e| e.to_string())
        },
        None,
    )
    .await
    .unwrap()
    .expect("a job to dispatch");
    assert_eq!(done.state, "succeeded");

    // The pipeline actually populated the per-org lake (base_10 => 10 entities).
    let entities = session
        .query_strings(&format!(
            "select count(distinct entity_id) from {}",
            er_cli::ENTITY_MEMBERSHIP
        ))
        .unwrap()[0][0]
        .clone();
    assert_eq!(
        entities, "10",
        "base_10 resolves to 10 entities in the lake"
    );

    // The queue reflects the success.
    let persisted = pg::get_job(&client, &oid(&client, "acme").await, &done.id)
        .await
        .unwrap()
        .expect("job present");
    assert_eq!(persisted.state, "succeeded");

    // Failure path: a runner error marks the job `failed`.
    let _ = pg::enqueue(
        &client,
        &oid(&client, "acme").await,
        "run_all_full",
        0,
        None,
        None,
    )
    .await
    .unwrap();
    let failed = dispatch_pg(&client, |_job| Err("boom".to_string()), None)
        .await
        .unwrap()
        .expect("a second job to dispatch");
    assert_eq!(failed.state, "failed");
    assert_eq!(
        pg::get_job(&client, &oid(&client, "acme").await, &failed.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "failed"
    );

    // Empty queue: nothing claimable -> None (runner never invoked).
    let none = dispatch_pg(
        &client,
        |_job| panic!("runner must not run on an empty queue"),
        None,
    )
    .await
    .unwrap();
    assert!(none.is_none(), "no job left to claim");

    let _ = std::fs::remove_dir_all(&tmp);
}
