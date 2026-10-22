//! The deploy's tenant sweep end to end (gated on BOTH `ER_TEST_PG_DSN` and
//! `DEDUPE_E2E_DUCKLAKE=1`): `migrate-tenants` enqueues one `lake_migrate` job
//! per active org, the dispatcher reconciles each lake and records its position,
//! and the report says `migrated`, then `current`, then `breaking` once a lake
//! has been retyped behind the reconciler's back — the job-start reconcile is
//! what every other lake kind runs too, so this is also the proof that a lake
//! job refuses to run against a table it does not fit. Run with:
//!   ER_TEST_PG_DSN='...' DEDUPE_E2E_DUCKLAKE=1 \
//!     cargo test -p er-server --test pg_migrate_tenants
//!
//! ONE test with sequential phases (the Postgres tier shares one schema).

use std::time::Duration;

use er_server::{dispatcher, pg, schema};

async fn oid(client: &tokio_postgres::Client, name: &str) -> String {
    pg::org_id_by_name(client, name)
        .await
        .expect("org lookup")
        .expect("org exists")
}

#[tokio::test]
async fn the_sweep_reconciles_every_active_tenant_and_reports_it() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping the tenant sweep test (set ER_TEST_PG_DSN to run)");
        return;
    };
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping the tenant sweep test (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS tenant_schema_states, staged_steward_actions, audit_log, schedules, config_versions, api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    // Two orgs: one active with a fresh local lake, one suspended (skipped).
    let tmp = std::env::temp_dir().join(format!("er_sweep_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();
    pg::ensure_org(&client, "acme", "/configs/acme.yaml")
        .await
        .unwrap();
    pg::ensure_org(&client, "dormant", "/configs/dormant.yaml")
        .await
        .unwrap();
    let acme = oid(&client, "acme").await;
    pg::merge_org_env(
        &client,
        "acme",
        &serde_json::json!({"ER_LAKE_CATALOG": catalog, "ER_LAKE_DATA_PATH": data_path}),
    )
    .await
    .unwrap();
    pg::set_org_state(&client, "dormant", "suspended", "active")
        .await
        .unwrap();

    // ---- phase 1: a fresh lake is migrated ---------------------------------
    let jobs = schema::enqueue_tenant_migrations(&client).await.unwrap();
    assert_eq!(jobs.len(), 1, "only the active org is swept: {jobs:?}");
    assert_eq!(jobs[0].0, "acme");
    assert_eq!(jobs[0].1.kind, schema::LAKE_MIGRATE_KIND);
    assert!(dispatcher::dispatch_once(&client).await.unwrap());
    let report = schema::wait_for_tenant_migrations(&client, &jobs, Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(report.len(), 1);
    assert_eq!(report[0].org, "acme");
    assert_eq!(report[0].outcome, "migrated", "{report:?}");
    assert!(!report[0].failed());
    assert!(
        report[0].detail.contains("lake.main.raw_records"),
        "{report:?}"
    );
    let states = er_entities::tenant_schema_state::read::list(&client)
        .await
        .unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].org_id, acme);
    assert_eq!(states[0].catalog_hash, schema::lake_catalog_hash());

    // ---- phase 2: a current lake reports current, and no second ledger row -
    let jobs = schema::enqueue_tenant_migrations(&client).await.unwrap();
    assert!(dispatcher::dispatch_once(&client).await.unwrap());
    let report = schema::wait_for_tenant_migrations(&client, &jobs, Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(report[0].outcome, "current", "{report:?}");
    let session = er_cli::open_lake(
        &tmp.join("catalog.ducklake").to_string_lossy(),
        &tmp.join("data").to_string_lossy(),
    )
    .unwrap();
    let verified = session
        .connection()
        .query_row(
            "SELECT count(*) FROM lake.main.schema_migrations WHERE kind = 'verified'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(
        verified, 0,
        "a migrated lake needs no verified row for the same version"
    );

    // ---- phase 3: a retyped lake column is breaking: the job fails, the ---
    //      report says so, and a run_all against that lake fails the same way
    session
        .connection()
        .execute_batch("ALTER TABLE lake.main.runs ALTER COLUMN snapshot_start TYPE DOUBLE;")
        .unwrap();
    drop(session);
    let jobs = schema::enqueue_tenant_migrations(&client).await.unwrap();
    assert!(dispatcher::dispatch_once(&client).await.unwrap());
    let report = schema::wait_for_tenant_migrations(&client, &jobs, Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(report[0].outcome, "breaking", "{report:?}");
    assert!(report[0].failed());
    assert!(
        report[0].detail.contains("ERR_SCHEMA_BREAKING"),
        "{report:?}"
    );
    assert!(report[0].detail.contains("snapshot_start"), "{report:?}");
    let job = pg::get_job(&client, &acme, &report[0].job_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        job.state, "failed",
        "a breaking schema is terminal, not retried"
    );
    assert_eq!(job.error_class.as_deref(), Some("internal"));

    let run = pg::enqueue(&client, &acme, "lake_maintain", 0, None, None, None)
        .await
        .unwrap();
    assert!(dispatcher::dispatch_once(&client).await.unwrap());
    let run = pg::get_job(&client, &acme, &run.id).await.unwrap().unwrap();
    assert_eq!(run.state, "failed", "every lake job reconciles first");
    assert!(
        run.error_detail
            .as_deref()
            .unwrap_or_default()
            .contains("ERR_SCHEMA_BREAKING"),
        "{run:?}"
    );

    let text = schema::render_report(&report);
    assert!(text.contains("acme"), "{text}");
    assert!(text.contains("breaking"), "{text}");

    let _ = std::fs::remove_dir_all(&tmp);
}
