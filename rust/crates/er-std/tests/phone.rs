//! Output parity of the `phone_e164` SQL generator against the real dbt macro.
//! The golden (tests/phone_golden.json) is produced by rendering + running the
//! dbt macro in DuckDB via the Python MacroHarness; here the Rust-generated SQL
//! runs in a dedupe-core session over the same inputs and must produce identical
//! `phone_e164` / `phone_valid`. Regenerate the golden with
//! `uv run python rust/crates/er-std/tests/gen_phone_golden.py`.

use dedupe_core::session::{Session, SessionOptions};
use er_std::sql::phone_e164;

/// A SQL VARCHAR literal for `value`, or `NULL`.
fn lit(value: Option<&str>) -> String {
    match value {
        None => "NULL".to_string(),
        Some(s) => format!("'{}'", s.replace('\'', "''")),
    }
}

/// `query_strings` renders NULL as the literal "NULL"; map a golden JSON value to
/// that same text (booleans as DuckDB prints them).
fn expected_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "NULL".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[test]
fn phone_e164_matches_dbt_macro() {
    let raw = include_str!("phone_golden.json");
    let golden: serde_json::Value = serde_json::from_str(raw).unwrap();
    let cases = golden["cases"].as_array().unwrap();
    assert!(cases.len() >= 20, "expected a broad case set");

    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();

    for case in cases {
        let input = case["input"].as_str();
        let region = case["region"].as_str().unwrap();
        let projection = phone_e164("col", region);
        let sql = format!(
            "select {projection} from (select cast({} as varchar) as col) as input",
            lit(input)
        );
        let rows = session
            .query_strings(&sql)
            .unwrap_or_else(|e| panic!("input {input:?} region {region}: {e}\nSQL: {sql}"));
        assert_eq!(rows.len(), 1);
        let got_e164 = &rows[0][0];
        let got_valid = &rows[0][1];
        assert_eq!(
            *got_e164,
            expected_text(&case["phone_e164"]),
            "phone_e164 for {input:?} ({region}) differs from the dbt macro",
        );
        assert_eq!(
            *got_valid,
            expected_text(&case["phone_valid"]),
            "phone_valid for {input:?} ({region}) differs from the dbt macro",
        );
    }
}
