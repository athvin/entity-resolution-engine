//! P2 CLI gate (behind `DEDUPE_E2E_DUCKLAKE=1`): drive the whole full-mode
//! pipeline through the `er run-all` BINARY against a real DuckLake catalog, then
//! re-open the lake and assert the committed base_10 relations reproduce — the
//! end-to-end demonstration that the Rust CLI replaces Python `er`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use assert_cmd::Command;

fn enabled() -> bool {
    std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() == Ok("1")
}

fn repo_fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/static")
        .join(rel)
}

fn crate_fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

const GOLDEN_COLUMNS: [&str; 13] = [
    "given_name",
    "family_name",
    "email",
    "phone_e164",
    "addr_number",
    "addr_street",
    "addr_unit",
    "addr_city",
    "addr_region",
    "addr_postal",
    "birth_date",
    "metadata",
    "survivorship_version",
];

#[test]
fn er_run_all_reproduces_base_10_relations() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("er_cli_runall_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let drop = tmp.join("drop");
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    std::fs::create_dir_all(&drop).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();

    // Assemble the drop dir from base_10's base delivery.
    for source in ["crm", "billing", "webforms"] {
        std::fs::copy(
            repo_fixture(&format!("base_10/base/{source}.csv")),
            drop.join(format!("{source}.csv")),
        )
        .unwrap();
    }

    Command::cargo_bin("er")
        .unwrap()
        .args([
            "--catalog",
            &catalog,
            "--data-path",
            &data_path,
            "--config",
            crate_fixture("test.yaml").to_str().unwrap(),
            "run-all",
            "--drop",
            drop.to_str().unwrap(),
            "--lexicon",
            crate_fixture("nickname_variants.csv").to_str().unwrap(),
            "--model",
            repo_fixture("model_test_v1.json").to_str().unwrap(),
            "--tf",
            repo_fixture("model_test_v1.tf.csv").to_str().unwrap(),
            "--model-version",
            "v0001",
            "--tf-snapshot-id",
            "01JWMDTFSNAP00000000000001",
            "--run-id",
            "01RUNALL00000000000000000",
        ])
        .assert()
        .success();

    // Re-open the lake read-only and verify the final relations.
    let session = er_cli::open_lake(&catalog, &data_path).unwrap();

    // Membership partitions into base_10's 10 entities.
    let mut by_entity: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for r in session
        .query_strings(&format!(
            "select entity_id, record_key from {}",
            er_cli::ENTITY_MEMBERSHIP
        ))
        .unwrap()
    {
        by_entity
            .entry(r[0].clone())
            .or_default()
            .insert(r[1].clone());
    }
    let got: BTreeSet<BTreeSet<String>> = by_entity.into_values().collect();

    let membership =
        std::fs::read_to_string(repo_fixture("base_10/expected/base/membership.csv")).unwrap();
    let mut want_by: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for line in membership.lines().skip(1).filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split(',').collect();
        want_by
            .entry(f[3].to_string())
            .or_default()
            .insert(format!("{}:{}", f[1], f[2]));
    }
    let want: BTreeSet<BTreeSet<String>> = want_by.into_values().collect();
    assert_eq!(
        got.len(),
        10,
        "run-all should resolve base_10 to 10 entities"
    );
    assert_eq!(
        got, want,
        "run-all membership partition differs from golden"
    );

    // ingest_batches carries one receipt per source: base_10 is all-new
    // (23 records across crm/billing/webforms), full_refresh_keys false.
    let receipts = session
        .query_strings(&format!(
            "select source_system, new_count, changed_count, unchanged_count, \
                    tombstone_count, resurrected_count, full_refresh_keys \
             from {} order by source_system",
            er_cli::INGEST_BATCHES
        ))
        .unwrap();
    assert_eq!(receipts.len(), 3, "one ingest_batches receipt per source");
    let total_new: i64 = receipts.iter().map(|r| r[1].parse::<i64>().unwrap()).sum();
    assert_eq!(total_new, 23, "every base_10 record lands as new");
    for r in &receipts {
        assert_eq!(
            (&r[2], &r[3], &r[4], &r[5], &r[6]),
            (
                &"0".to_string(),
                &"0".to_string(),
                &"0".to_string(),
                &"0".to_string(),
                &"false".to_string()
            ),
            "base delivery has no changed/unchanged/tombstone/resurrected and is not a full refresh ({})",
            r[0]
        );
    }

    // golden_records match golden.csv (attribute columns, id-insensitive).
    let golden_path = repo_fixture("base_10/expected/base/golden.csv")
        .to_string_lossy()
        .replace('\'', "''");
    let cols_decl = GOLDEN_COLUMNS
        .iter()
        .map(|c| format!("'{c}': 'VARCHAR'"))
        .collect::<Vec<_>>()
        .join(", ");
    session
        .execute_batch(&format!(
            "create temp table golden_expected as select * from read_csv('{golden_path}', \
             header = true, nullstr = '\\N', columns = {{'entity_label': 'VARCHAR', {cols_decl}}})"
        ))
        .unwrap();
    let projection = GOLDEN_COLUMNS
        .iter()
        .map(|c| format!("cast({c} as varchar) as {c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let cols = GOLDEN_COLUMNS.join(", ");
    let diff = session
        .count(&format!(
            "((select {projection} from {gr} except select {cols} from golden_expected) \
              union all \
              (select {cols} from golden_expected except select {projection} from {gr}))",
            gr = er_cli::GOLDEN_RECORDS,
        ))
        .unwrap();
    assert_eq!(
        diff, 0,
        "{diff} run-all golden_records rows differ from golden.csv"
    );

    // `er doctor` reports the populated lake as healthy. Drop our session first
    // so the separate `er` process can attach the (single-writer) catalog.
    // (`drop` the local var shadows the fn, so name it fully.)
    std::mem::drop(session);
    let out = Command::cargo_bin("er")
        .unwrap()
        .args([
            "--catalog",
            &catalog,
            "--data-path",
            &data_path,
            "--config",
            crate_fixture("test.yaml").to_str().unwrap(),
            "doctor",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "er doctor failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).expect("doctor emits JSON");
    assert_eq!(report["ok"], true, "a fully-run lake is ok");
    assert_eq!(report["latest_run"]["status"], "succeeded");
    let rel = |name: &str| -> &serde_json::Value {
        report["relations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap()
    };
    assert_eq!(
        rel("int_std_records")["rows"],
        23,
        "23 standardized records"
    );
    assert_eq!(rel("entity_membership")["present"], true);
    assert_eq!(rel("golden_records")["present"], true);

    // `er maintain` flushes + compacts + expires the lake and reports before/after
    // file counts. "100 years" expiry keeps every snapshot (just exercises the
    // call); the real signal is that maintenance runs clean over a live lake.
    let out = Command::cargo_bin("er")
        .unwrap()
        .args([
            "--catalog",
            &catalog,
            "--data-path",
            &data_path,
            "--config",
            crate_fixture("test.yaml").to_str().unwrap(),
            "maintain",
            "--expire-older-than",
            "100 years",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "er maintain failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let m: serde_json::Value = serde_json::from_slice(&out.stdout).expect("maintain emits JSON");
    assert_eq!(m["expired_older_than"], "100 years");
    let after = m["after"].as_array().expect("after stats");
    assert!(
        after.iter().any(|t| t["table"] == "golden_records"),
        "maintenance stats cover the lake tables"
    );
    // Every table has at least one data file once inlined rows are flushed.
    assert!(
        after
            .iter()
            .all(|t| t["file_count"].as_i64().unwrap_or(-1) >= 1),
        "flush produced a data file per table: {after:?}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
