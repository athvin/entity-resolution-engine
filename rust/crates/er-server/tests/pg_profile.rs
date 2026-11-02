//! The `profile` job kind through the real queue (gated on BOTH
//! `ER_TEST_PG_DSN` and `DEDUPE_E2E_DUCKLAKE=1`): a job claimed from Postgres
//! runs a profile over a per-org DuckLake and the outcome is written back.
//!
//! ONE test with sequential phases, per `pg_serial.rs` — `common::fresh_db` is
//! keyed on one slug per file, so a second test would ask for the same database
//! and its `DROP DATABASE ... WITH (FORCE)` would disconnect the first mid-test.
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='...' DEDUPE_E2E_DUCKLAKE=1 \
//!     cargo test -p er-server --test main pg_profile

use std::path::PathBuf;

use er_server::profile_params::ProfileRequest;
use er_server::{dispatch_pg, pg};

use crate::common;

const TEST_YAML: &str = include_str!("../../er-cli/tests/fixtures/test.yaml");
const NICKNAMES: &str = include_str!("../../er-cli/tests/fixtures/nickname_variants.csv");

async fn oid(client: &tokio_postgres::Client, name: &str) -> String {
    pg::org_id_by_name(client, name)
        .await
        .expect("org lookup")
        .expect("org exists")
}

fn repo_fixture(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/static")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

fn lake_count(session: &dedupe_core::Session, relation: &str) -> i64 {
    session
        .query_strings(&format!("select count(*) from lake.main.{relation}"))
        .unwrap()[0][0]
        .parse()
        .unwrap()
}

#[tokio::test]
async fn a_profile_job_runs_through_the_queue_and_lands_in_the_lake() {
    let Some(db) = common::fresh_db("profile").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping profile dispatch e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::ensure_org(&client, "acme", "/configs/acme.yaml")
        .await
        .unwrap();
    let org = oid(&client, "acme").await;

    // A corpus to profile: ingest + standardize base_10 into a per-org lake.
    let tmp = std::env::temp_dir().join(format!("er_profile_{}", std::process::id()));
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
    er_cli::seed_lexicon(&session, &lexicon).unwrap();
    for source in ["crm", "billing", "webforms"] {
        let csv =
            std::fs::read_to_string(repo_fixture(&format!("base_10/base/{source}.csv"))).unwrap();
        er_cli::run_ingest(
            &session,
            &config,
            source,
            &csv,
            "01BATCH0000000000000000BASE",
            "2026-01-01 00:00:00",
            false,
        )
        .unwrap();
    }
    er_cli::run_standardize(&session, &config, "2026-01-01 00:00:00").unwrap();

    // Phase 1: a queued `profile` job is claimed and run, and its rows land.
    let params = serde_json::json!({"subject": "standardized", "tier": 1});
    let queued = pg::enqueue(
        &client,
        &org,
        None,
        "profile",
        0,
        None,
        Some(&params),
        pg::Provenance::default(),
    )
    .await
    .unwrap();

    let done = dispatch_pg(
        &client,
        |job| {
            assert_eq!(job.id, queued.id, "claimed the queued job");
            assert_eq!(job.kind, "profile");
            let request = ProfileRequest::from_params(&job.params)?;
            request
                .run(&session, false, "2026-01-02 00:00:00")
                .map(|_| ())
        },
        None,
    )
    .await
    .unwrap()
    .expect("a job to dispatch");
    assert_eq!(done.state, "succeeded");

    assert_eq!(
        lake_count(&session, "profile_runs"),
        1,
        "the profile run was recorded"
    );
    let columns = lake_count(&session, "column_profiles");
    assert!(columns > 0, "every profiled column has a row");

    // Phase 2: the same job again is the SAME profile. The content digest is the
    // cache key, so an unchanged corpus does not accumulate rows — and this is
    // the path a schedule takes, firing repeatedly over data that mostly has not
    // moved.
    let _ = pg::enqueue(
        &client,
        &org,
        None,
        "profile",
        0,
        None,
        Some(&params),
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    let again = dispatch_pg(
        &client,
        |job| {
            ProfileRequest::from_params(&job.params)?
                .run(&session, false, "2026-01-03 00:00:00")
                .map(|_| ())
        },
        None,
    )
    .await
    .unwrap()
    .expect("a second job");
    assert_eq!(again.state, "succeeded");
    assert_eq!(
        lake_count(&session, "profile_runs"),
        1,
        "re-profiling an unchanged corpus rewrites its row rather than appending"
    );
    assert_eq!(
        lake_count(&session, "column_profiles"),
        columns,
        "nor does it duplicate the per-column rows"
    );

    let raw_params = serde_json::json!({"subject": "raw"});
    // Phase 3: bad params FAIL the job with the reason, rather than panicking
    // the dispatcher or succeeding vacuously. A raw profile needs a source.
    let _ = pg::enqueue(
        &client,
        &org,
        None,
        "profile",
        0,
        None,
        Some(&raw_params),
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    let refused = dispatch_pg(
        &client,
        |job| {
            ProfileRequest::from_params(&job.params)?
                .run(&session, false, "2026-01-04 00:00:00")
                .map(|_| ())
        },
        None,
    )
    .await
    .unwrap()
    .expect("a third job");
    assert_eq!(refused.state, "failed");
    let persisted = pg::get_job(&client, &org, &refused.id)
        .await
        .unwrap()
        .expect("the refused job is in the queue");
    assert_eq!(persisted.state, "failed", "the queue records the refusal");
    // The failure REASON is deliberately not asserted here. `dispatch_pg` marks
    // the terminal state with `pg::mark` and discards the runner's error string
    // — only the production loop routes it through `fail_or_retry`, which is
    // what populates `error_class`/`error_detail`. So a test on this helper can
    // observe that a bad request fails, never why. The message itself is pinned
    // where it is produced, in `profile_params`' own tests.
    assert!(
        persisted.error_detail.is_none(),
        "dispatch_pg does not record a reason; if it starts to, assert on it here"
    );
    assert_eq!(
        lake_count(&session, "profile_runs"),
        1,
        "a refused profile writes nothing"
    );

    // Phase 4: `profile` is schedulable, and the scheduler's bare `{}` is a
    // valid request — the pairing `dispatch_coverage.rs` checks structurally,
    // asserted here against the parser the dispatcher actually uses.
    assert!(
        er_server::SCHEDULABLE_KINDS.contains(&"profile"),
        "profile must be schedulable for a drift series to exist"
    );
    let defaulted = ProfileRequest::from_params(&serde_json::json!({})).unwrap();
    assert_eq!(defaulted.tier, 1);
    assert!(defaulted.options.exact_distinct);
}
