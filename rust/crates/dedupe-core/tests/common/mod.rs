//! Shared helpers for golden-fixture regression tests. Each integration-test
//! binary compiles this module but uses only the helpers it needs.
#![allow(dead_code)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/golden/fake_1000")
}

pub fn load_golden_json(name: &str) -> serde_json::Value {
    let path = golden_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read golden {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse golden {name}: {e}"))
}

pub fn load_golden_text(name: &str) -> String {
    let path = golden_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read golden {}: {e}", path.display()))
}

/// Schema matching how the golden generator ingests fake_1000 via pandas
/// (strings stay strings — notably `dob`).
pub const FAKE_1000_SCHEMA: &[(&str, &str)] = &[
    ("unique_id", "BIGINT"),
    ("first_name", "VARCHAR"),
    ("surname", "VARCHAR"),
    ("dob", "VARCHAR"),
    ("city", "VARCHAR"),
    ("email", "VARCHAR"),
    ("cluster", "BIGINT"),
];

pub fn fake_1000_csv_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/datasets/fake_1000.csv")
}

use dedupe_core::session::{Session, TableHandle};

/// Diff a materialized table against a golden parquet INSIDE DuckDB.
/// `keys` must uniquely identify rows on both sides and be non-null.
/// `exact` columns compare with IS DISTINCT FROM; `approx` columns compare
/// with an absolute tolerance (NULL-ness must match).
pub fn assert_table_matches_parquet(
    session: &Session,
    actual: &TableHandle,
    golden_file: &str,
    keys: &[&str],
    exact: &[&str],
    approx: &[(&str, f64)],
) {
    assert_table_matches_parquet_path(
        session,
        actual,
        &golden_dir().join(golden_file),
        keys,
        exact,
        approx,
    );
}

/// As `assert_table_matches_parquet`, but with an absolute golden path (for
/// datasets outside the default fake_1000 golden dir).
pub fn assert_table_matches_parquet_path(
    session: &Session,
    actual: &TableHandle,
    golden: &Path,
    keys: &[&str],
    exact: &[&str],
    approx: &[(&str, f64)],
) {
    let golden_str = golden.to_string_lossy().replace('\'', "''");
    let golden_file = golden
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let actual_count = session.count(&actual.physical).unwrap();
    let golden_count = session
        .count(&format!("read_parquet('{golden_str}')"))
        .unwrap();
    assert_eq!(
        actual_count, golden_count,
        "{golden_file}: row count mismatch (actual {actual_count} vs golden {golden_count})"
    );

    let join_on = keys
        .iter()
        .map(|k| format!("a.\"{k}\" = g.\"{k}\""))
        .collect::<Vec<_>>()
        .join(" AND ");
    let mut mismatch_preds: Vec<(String, String)> = Vec::new();
    let k0 = keys[0];
    mismatch_preds.push((
        "row_set".to_string(),
        format!("a.\"{k0}\" IS NULL OR g.\"{k0}\" IS NULL"),
    ));
    for c in exact {
        mismatch_preds.push((
            c.to_string(),
            format!("a.\"{c}\" IS DISTINCT FROM g.\"{c}\""),
        ));
    }
    for (c, tol) in approx {
        mismatch_preds.push((
            c.to_string(),
            format!(
                "(CASE WHEN a.\"{c}\" IS NULL AND g.\"{c}\" IS NULL THEN false \
                 WHEN a.\"{c}\" IS NULL OR g.\"{c}\" IS NULL THEN true \
                 ELSE abs(a.\"{c}\" - g.\"{c}\") > {tol:e} END)"
            ),
        ));
    }

    let any_pred = mismatch_preds
        .iter()
        .map(|(_, p)| format!("({p})"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let base = format!(
        "FROM {} a FULL OUTER JOIN read_parquet('{golden_str}') g ON {join_on}",
        actual.physical
    );
    let total: i64 = session
        .query_scalar_f64(&format!("SELECT count(*)::DOUBLE {base} WHERE {any_pred}"))
        .unwrap() as i64;
    if total == 0 {
        return;
    }

    // Diagnose: per-predicate mismatch counts, then sample rows.
    let mut report = format!("{golden_file}: {total} mismatching rows\n");
    for (name, pred) in &mismatch_preds {
        let n = session
            .query_scalar_f64(&format!("SELECT count(*)::DOUBLE {base} WHERE {pred}"))
            .unwrap() as i64;
        if n > 0 {
            let _ = writeln!(report, "  column {name}: {n} mismatches");
            let key_sel = keys
                .iter()
                .map(|k| format!("coalesce(a.\"{k}\", g.\"{k}\") as \"{k}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let val_sel = if name == "row_set" {
                format!("a.\"{k0}\" IS NULL as missing_in_actual")
            } else {
                format!("a.\"{name}\" as actual_value, g.\"{name}\" as golden_value")
            };
            let rows = session
                .query_strings(&format!(
                    "SELECT {key_sel}, {val_sel} {base} WHERE {pred} LIMIT 5"
                ))
                .unwrap();
            for row in rows {
                let _ = writeln!(report, "    {}", row.join(" | "));
            }
        }
    }
    panic!("{report}");
}
