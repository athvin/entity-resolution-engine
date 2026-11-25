//! #1229: a run's results stay readable at the snapshot the run recorded, after
//! a newer run has moved the lake (behind `DEDUPE_E2E_DUCKLAKE=1`).
//!
//! Two full runs over the account fixture. The second adds one company the
//! first never saw, so the golden count differs between them: four after run
//! R, five after run R+1. Then:
//!
//! 1. both runs carry a `snapshot`, and R's precedes R+1's;
//! 2. reading golden records and duplicate groups at R's snapshot gives R's
//!    results, not the latest — the read a customer makes after `job.succeeded`
//!    for R arrives late;
//! 3. once retention expires R's snapshot, asking for it is refused as
//!    `Expired`, rather than answered from a different snapshot.

use std::path::{Path, PathBuf};

use arrow::record_batch::RecordBatch;
use er_config::{EntityType, load_config_str};
use er_server::read;

fn enabled() -> bool {
    std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() == Ok("1")
}

fn repo_fixture(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/static")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

fn golden_rows(session: &dedupe_core::session::Session, snapshot: i64) -> usize {
    read::golden_arrow_page(session, EntityType::Account, snapshot, None, None, 50)
        .unwrap()
        .batches
        .iter()
        .map(RecordBatch::num_rows)
        .sum()
}

#[test]
fn a_runs_results_read_at_its_snapshot_after_a_newer_run() {
    if !enabled() {
        eprintln!("skipping run-snapshot read e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("er_run_snapshot_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();
    let session = er_cli::open_lake(&catalog, &data_path).unwrap();
    let config =
        load_config_str(&std::fs::read_to_string(repo_fixture("acct_base/config.yaml")).unwrap())
            .unwrap();
    let crm = std::fs::read_to_string(repo_fixture("acct_base/crm.csv")).unwrap();
    let vendors = std::fs::read_to_string(repo_fixture("acct_base/vendors.csv")).unwrap();
    let model_json = std::fs::read_to_string(repo_fixture("acct_model_v1.json")).unwrap();
    let tf_csv = repo_fixture("acct_model_v1.tf.csv");
    let run = |run_id: &str, day: u8, crm: &str| {
        let deliveries = vec![
            ("crm".to_string(), crm.to_string()),
            ("vendors".to_string(), vendors.clone()),
        ];
        er_cli::run_all(
            &session,
            &config,
            &er_cli::PipelineInputs {
                mode: "full",
                rebuild_reason: None,
                segment: None,
                lexicon: &Vec::new(),
                ignore_words: &[],
                deliveries: &deliveries,
                model: &er_cli::MatchModel {
                    settings_json: &model_json,
                    tf_csv_path: Path::new(&tf_csv),
                    model_version: "acct_v1",
                    tf_snapshot_id: "ACCTTFSNAP0000000000000001",
                },
                run_id,
                ingested_at: &format!("2026-03-{day:02} 00:00:00"),
                scored_at: &format!("2026-03-{day:02} 01:00:00"),
                run_at: &format!("2026-03-{day:02} 02:00:00"),
            },
        )
        .unwrap();
    };

    run("01RUNSNAPR0000000000000000", 1, &crm);
    // Run R+1 sees one more company, unlike anything already in the corpus.
    let more = format!(
        "{}c9,Hooli Systems,https://hooli.example,212-555-0999,99 Unique Way,Faraway,WA,98001,2026-01-09\n",
        crm.trim_end_matches('\n').to_string() + "\n"
    );
    run("01RUNSNAPR1000000000000000", 5, &more);

    // 1. Both runs recorded the snapshot holding their results.
    let latest = read::latest_snapshot(&session).unwrap();
    let runs = read::runs_list(&session, latest, None).unwrap();
    let snapshot_of = |id: &str| {
        runs.iter()
            .find(|r| r.id == id)
            .unwrap_or_else(|| panic!("no run {id}"))
            .snapshot
            .unwrap_or_else(|| panic!("run {id} recorded no snapshot"))
    };
    let r = snapshot_of("01RUNSNAPR0000000000000000");
    let r1 = snapshot_of("01RUNSNAPR1000000000000000");
    assert!(r < r1, "run R's snapshot {r} must precede R+1's {r1}");

    // 2. R's results at R's snapshot; R+1's at the latest.
    assert_eq!(
        golden_rows(&session, r),
        4,
        "run R's golden records at its snapshot"
    );
    assert_eq!(
        golden_rows(&session, latest),
        5,
        "run R+1 added one company"
    );
    let at_r = read::duplicates(
        &session,
        EntityType::Account,
        r,
        None,
        &read::DuplicateFilters::default(),
        50,
    )
    .unwrap()
    .0;
    assert_eq!(at_r.len(), 3, "run R's duplicate groups at its snapshot");
    let bounds = || session.snapshot_bounds().map_err(|e| e.to_string());
    assert_eq!(
        read::pin_requested((latest, None), Some(r), bounds),
        Ok((r, None))
    );

    // 3. Expire everything but the newest snapshot; R's is now gone, and the
    //    request for it is refused rather than served from another snapshot.
    session.expire_snapshots("0 seconds").unwrap();
    let (oldest, _) = session.snapshot_bounds().unwrap();
    assert!(
        oldest > r,
        "expiry left snapshot {r} retained (oldest {oldest})"
    );
    assert_eq!(
        read::pin_requested((latest, None), Some(r), || session
            .snapshot_bounds()
            .map_err(|e| e.to_string())),
        Err(read::PinError::Expired {
            requested: r,
            oldest
        })
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
