//! P2 end-to-end gates (behind `DEDUPE_E2E_DUCKLAKE=1`, like dedupe-cli's
//! cli_e2e, since they install the `ducklake` extension): run the pipeline
//! stages over a REAL DuckLake catalog and assert the lake relations match
//! base_10's committed goldens — the same oracles the in-memory stage gates use,
//! now proving the lake orchestration end to end.

use std::collections::BTreeMap;
use std::path::PathBuf;

use dedupe_core::Session;
use er_config::{Config, load_config_str};
use er_hash::std_hash::{STD_HASH_COLUMNS, StdValue, table_content_hash};

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

/// Open a fresh lake under a per-test temp dir (tests run in parallel) and run
/// ingest + standardize over base_10's base delivery. Returns the session, the
/// config, and the temp dir to clean up.
fn standardized_lake(tag: &str) -> (Session, Config, PathBuf) {
    let tmp = std::env::temp_dir().join(format!("er_cli_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();

    let session = er_cli::open_lake(&catalog, &data_path).unwrap();
    let config = load_config_str(include_str!("fixtures/test.yaml")).unwrap();

    let lexicon: Vec<(String, String)> = include_str!("fixtures/nickname_variants.csv")
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
    er_cli::run_standardize(&session, &config).unwrap();
    (session, config, tmp)
}

/// Mirror of er-standardize's std_hashes gate column render.
fn render_col(column: &str) -> String {
    match column {
        "name_variants" => "array_to_string(name_variants, chr(31))".to_string(),
        "email_valid" | "phone_valid" | "birth_date" | "updated_at_source" => {
            format!("coalesce(cast({column} as varchar), '')")
        }
        _ => format!("coalesce({column}, '')"),
    }
}

#[test]
fn ingest_and_standardize_through_ducklake_match_golden() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, _config, tmp) = standardized_lake("std");

    let select_cols: Vec<String> = STD_HASH_COLUMNS.iter().map(|c| render_col(c)).collect();
    let rows = session
        .query_strings(&format!(
            "select source_system, source_record_id, {} from {}",
            select_cols.join(", "),
            er_cli::INT_STD_RECORDS
        ))
        .unwrap();
    let mut computed: BTreeMap<(String, String), String> = BTreeMap::new();
    for row in &rows {
        let mut vals: BTreeMap<String, StdValue> = BTreeMap::new();
        for (i, column) in STD_HASH_COLUMNS.iter().enumerate() {
            vals.insert((*column).to_string(), StdValue::Text(row[2 + i].clone()));
        }
        computed.insert(
            (row[0].clone(), row[1].clone()),
            table_content_hash(&vals).unwrap(),
        );
    }

    let golden =
        std::fs::read_to_string(repo_fixture("base_10/expected/base/std_hashes.csv")).unwrap();
    let mut expected: BTreeMap<(String, String), String> = BTreeMap::new();
    for line in golden.lines().skip(1).filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split(',').collect();
        expected.insert((f[0].to_string(), f[1].to_string()), f[2].to_string());
    }

    assert_eq!(
        computed.len(),
        23,
        "expected 23 current std rows in the lake"
    );
    assert_eq!(
        computed, expected,
        "lake int_std_records std_hashes differ from the committed golden"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn match_through_ducklake_matches_golden() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, _config, tmp) = standardized_lake("match");

    let model_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let tf_csv = repo_fixture("model_test_v1.tf.csv");
    er_cli::run_match(
        &session,
        &er_cli::MatchModel {
            settings_json: &model_json,
            tf_csv_path: std::path::Path::new(&tf_csv),
            model_version: "v0001",
            tf_snapshot_id: "01JWMDTFSNAP00000000000001",
        },
        0.60, // review_low
        "01RUNMATCH0000000000000000",
        "2026-01-02 00:00:00",
    )
    .unwrap();

    // Load the committed golden (rec_a_key, rec_b_key, match_probability) and
    // compare in SQL (DOUBLE vs DOUBLE) at 1e-10.
    let golden_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/match_scores_base_10.csv")
        .to_string_lossy()
        .replace('\'', "''");
    session
        .execute_batch(&format!(
            "create temp table golden as select * from read_csv('{golden_path}', header = true, \
             columns = {{'rec_a_key': 'VARCHAR', 'rec_b_key': 'VARCHAR', 'match_probability': 'DOUBLE'}})"
        ))
        .unwrap();

    let lake_n = session
        .query_strings(&format!("select count(*) from {}", er_cli::MATCH_SCORES))
        .unwrap()[0][0]
        .clone();
    assert_eq!(
        lake_n, "19",
        "expected 19 scored pairs in lake.match_scores"
    );

    let diff = session
        .count(&format!(
            "(
               (select rec_a_key, rec_b_key from {ms} except select rec_a_key, rec_b_key from golden)
               union all
               (select rec_a_key, rec_b_key from golden except select rec_a_key, rec_b_key from {ms})
             )",
            ms = er_cli::MATCH_SCORES,
        ))
        .unwrap();
    assert_eq!(
        diff, 0,
        "{diff} canonical pairs differ from the match golden"
    );

    let max_abs_diff = session
        .query_scalar_f64(&format!(
            "select coalesce(max(abs(m.match_probability - g.match_probability)), 0.0) \
             from {ms} m join golden g using (rec_a_key, rec_b_key)",
            ms = er_cli::MATCH_SCORES,
        ))
        .unwrap();
    assert!(
        max_abs_diff <= 1e-10,
        "max |match_probability - golden| = {max_abs_diff:e} exceeds 1e-10"
    );

    // The evidence waterfall is populated (NOT NULL) for every scored pair.
    let null_evidence = session
        .count(&format!(
            "(select 1 from {ms} where evidence is null)",
            ms = er_cli::MATCH_SCORES
        ))
        .unwrap();
    assert_eq!(null_evidence, 0, "every scored pair must carry evidence");

    let _ = std::fs::remove_dir_all(&tmp);
}
