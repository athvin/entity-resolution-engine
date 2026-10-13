//! The end-to-end standardization gate: ingest base_10's base delivery, build
//! stg_records -> int_std_records in a dedupe-core session, compute each row's
//! std_hash, and assert it matches the committed
//! fixtures/static/base_10/expected/base/std_hashes.csv byte-for-byte.

use std::collections::BTreeMap;

use dedupe_core::session::{Session, SessionOptions};
use er_config::load_config_str;
use er_hash::std_hash::{STD_HASH_COLUMNS, StdValue, table_content_hash};
use er_ingest::landing::{create_raw_records_sql, land_base_delivery};
use er_standardize::build_int_std_records;

/// Render one STD_HASH column to the exact text T-STD-1 hashes (NULL and "" both
/// become ""; bool/date/timestamp cast to varchar; the list joined with \x1f).
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
fn base_10_std_hashes_match_fixture() {
    let config = load_config_str(include_str!("fixtures/test.yaml")).unwrap();
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    session
        .execute_batch(&create_raw_records_sql("raw_records"))
        .unwrap();

    // Seed the nickname lexicon (active on every packaged row), the relation
    // name_variants reads.
    let values: Vec<String> = include_str!("fixtures/nickname_variants.csv")
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|line| {
            let (a, b) = line.split_once(',').unwrap();
            format!(
                "('{}', '{}', true)",
                a.replace('\'', "''"),
                b.replace('\'', "''")
            )
        })
        .collect();
    session
        .execute_batch(&format!(
            "create table nickname_variants as select * from (values {}) \
             as t(variant_a, variant_b, active)",
            values.join(", ")
        ))
        .unwrap();

    // Land base_10's base delivery.
    for (source, csv_text) in [
        ("crm", include_str!("fixtures/base_10/base/crm.csv")),
        ("billing", include_str!("fixtures/base_10/base/billing.csv")),
        (
            "webforms",
            include_str!("fixtures/base_10/base/webforms.csv"),
        ),
    ] {
        let spec = &config.sources[source];
        land_base_delivery(
            &session,
            "raw_records",
            source,
            spec,
            csv_text,
            "01BATCH0000000000000000BASE",
            "2026-01-01 00:00:00",
        )
        .unwrap();
    }

    // Standardize.
    build_int_std_records(
        &session,
        &config,
        "raw_records",
        "nickname_variants",
        "int_std_records",
    )
    .unwrap();

    // std_hash per row, computed from the pre-rendered columns.
    let select_cols: Vec<String> = STD_HASH_COLUMNS.iter().map(|c| render_col(c)).collect();
    let rows = session
        .query_strings(&format!(
            "select source_system, source_record_id, {} from int_std_records",
            select_cols.join(", ")
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

    // Expected from the committed fixture.
    let mut expected: BTreeMap<(String, String), String> = BTreeMap::new();
    for line in include_str!("fixtures/std_hashes.csv")
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
    {
        let f: Vec<&str> = line.split(',').collect();
        expected.insert((f[0].to_string(), f[1].to_string()), f[2].to_string());
    }

    assert_eq!(computed.len(), 23, "expected 23 current std rows");
    let mismatches: Vec<String> = expected
        .iter()
        .filter(|(k, v)| computed.get(*k) != Some(*v))
        .map(|(k, v)| format!("{k:?}: want {v}, got {:?}", computed.get(k)))
        .collect();
    assert!(
        mismatches.is_empty(),
        "{}/{} std_hashes differ:\n{}",
        mismatches.len(),
        expected.len(),
        mismatches
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
