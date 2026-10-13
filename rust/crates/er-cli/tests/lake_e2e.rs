//! P2 end-to-end gate (gated behind `DEDUPE_E2E_DUCKLAKE=1`, like dedupe-cli's
//! cli_e2e, since it installs the `ducklake` extension): run ingest + standardize
//! over a REAL DuckLake catalog and assert `lake.main.int_std_records`'s std_hash
//! per row matches base_10's committed golden — the same oracle er-standardize's
//! in-memory gate uses, now proving the lake orchestration end to end.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use er_config::load_config_str;
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

/// Mirror of er-standardize's std_hashes gate: render one STD_HASH column to the
/// exact text T-STD-1 hashes.
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

    let tmp = std::env::temp_dir().join(format!("er_cli_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();

    let session = er_cli::open_lake(&catalog, &data_path).unwrap();
    let config = load_config_str(include_str!("fixtures/test.yaml")).unwrap();

    // Seed the nickname lexicon from the vendored fixture.
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

    // Ingest base_10's base delivery (read from the canonical repo fixtures).
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

    // std_hash per lake row, from the pre-rendered columns.
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

    // Expected from the committed golden.
    let golden = std::fs::read_to_string(Path::new(&repo_fixture(
        "base_10/expected/base/std_hashes.csv",
    )))
    .unwrap();
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
