//! Output parity of the `email_norm` and `name_norm` SQL generators against the
//! real dbt macros. Goldens (email_golden.json / name_golden.json) are produced
//! by rendering+running the macros in DuckDB via the Python MacroHarness; here the
//! Rust-generated SQL runs in a dedupe-core session over the same inputs and must
//! match. Regenerate with `uv run python rust/crates/er-std/tests/gen_std_golden.py`.

use dedupe_core::session::{Session, SessionOptions};
use er_std::sql::{
    address_parse, email_norm, name_norm, name_variants, parse_date, source_metadata,
};

fn lit(value: Option<&str>) -> String {
    match value {
        None => "NULL".to_string(),
        Some(s) => format!("'{}'", s.replace('\'', "''")),
    }
}

fn expected_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "NULL".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[test]
fn email_norm_matches_dbt_macro() {
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("email_golden.json")).unwrap();
    let cases = golden["cases"].as_array().unwrap();
    assert!(cases.len() >= 20);
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();

    for case in cases {
        let input = case["input"].as_str();
        let strip_plus = case["strip_plus"].as_bool().unwrap();
        let placeholders: Vec<&str> = case["placeholders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap())
            .collect();
        let projection = email_norm("col", strip_plus, &placeholders);
        let sql = format!(
            "select {projection} from (select cast({} as varchar) as col) as input",
            lit(input)
        );
        let rows = session.query_strings(&sql).unwrap();
        assert_eq!(
            rows[0][0],
            expected_text(&case["email"]),
            "email for {input:?} (strip_plus={strip_plus}) differs",
        );
        assert_eq!(
            rows[0][1],
            expected_text(&case["email_valid"]),
            "email_valid for {input:?} (strip_plus={strip_plus}) differs",
        );
    }
}

#[test]
fn name_norm_matches_dbt_macro() {
    let golden: serde_json::Value = serde_json::from_str(include_str!("name_golden.json")).unwrap();
    let cases = golden["cases"].as_array().unwrap();
    assert!(cases.len() >= 10);
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();

    for case in cases {
        let input = case["input"].as_str();
        let projection = name_norm("col");
        let sql = format!(
            "select {projection} as r from (select cast({} as varchar) as col) as input",
            lit(input)
        );
        let rows = session.query_strings(&sql).unwrap();
        assert_eq!(
            rows[0][0],
            expected_text(&case["name"]),
            "name_norm for {input:?} differs from the dbt macro",
        );
    }
}

#[test]
fn parse_date_matches_dbt_macro() {
    let golden: serde_json::Value = serde_json::from_str(include_str!("date_golden.json")).unwrap();
    let cases = golden["cases"].as_array().unwrap();
    assert!(cases.len() >= 10);
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();

    for case in cases {
        let input = case["input"].as_str();
        let fmt = case["fmt"].as_str().unwrap();
        // Cast the DATE to VARCHAR so it renders as ISO text (query_strings has no
        // DATE arm); DuckDB DATE->VARCHAR is "YYYY-MM-DD", matching str(date).
        let projection = parse_date("col", fmt);
        let sql = format!(
            "select cast(({projection}) as varchar) as r \
             from (select cast({} as varchar) as col) as input",
            lit(input)
        );
        let rows = session.query_strings(&sql).unwrap();
        assert_eq!(
            rows[0][0],
            expected_text(&case["birth_date"]),
            "parse_date for {input:?} ({fmt}) differs from the dbt macro",
        );
    }
}

#[test]
fn source_metadata_matches_dbt_macro() {
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("metadata_golden.json")).unwrap();
    let cases = golden["cases"].as_array().unwrap();
    let count = cases.len();
    assert!(count > 0, "expected metadata cases");
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();

    for case in cases {
        let payload = case["payload"].as_str().unwrap();
        let excluded: Vec<&str> = case["excluded"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e.as_str().unwrap())
            .collect();
        // Cast the JSON result to VARCHAR so it renders as text in query_strings.
        let projection = source_metadata("col", &excluded);
        let sql = format!(
            "select cast(({projection}) as varchar) as r \
             from (select cast({} as varchar) as col) as input",
            lit(Some(payload))
        );
        let rows = session.query_strings(&sql).unwrap();
        assert_eq!(
            rows[0][0],
            expected_text(&case["metadata"]),
            "source_metadata differs from the dbt macro for payload {payload}",
        );
    }
}

#[test]
fn name_variants_matches_dbt_macro() {
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("variants_golden.json")).unwrap();
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();

    // Build the nickname lexicon table from the golden's committed seed pairs.
    let values: Vec<String> = golden["lexicon"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let a = p[0].as_str().unwrap().replace('\'', "''");
            let b = p[1].as_str().unwrap().replace('\'', "''");
            format!("('{a}', '{b}', true)")
        })
        .collect();
    session
        .execute_batch(&format!(
            "create table nickname_lex as select * from (values {}) \
             as t(variant_a, variant_b, active)",
            values.join(", ")
        ))
        .unwrap();

    let cases = golden["cases"].as_array().unwrap();
    let count = cases.len();
    assert!(count > 10, "expected a broad case set");
    for case in cases {
        let input = case["input"].as_str();
        // array_to_string with chr(31) == the \x1f form std_hash hashes; empty list -> "".
        let projection = name_variants("col", "nickname_lex");
        let sql = format!(
            "select array_to_string(({projection}), chr(31)) as r \
             from (select cast({} as varchar) as col) as input",
            lit(input)
        );
        let rows = session.query_strings(&sql).unwrap();
        assert_eq!(
            rows[0][0],
            expected_text(&case["name_variants"]),
            "name_variants for {input:?} differs from the dbt macro",
        );
    }
}

#[test]
fn address_parse_matches_grammar_golden() {
    // Reuse the RegexV1Parser golden: the SQL generator sharing the grammar's
    // pattern constants means a pass proves SQL == Rust grammar == (transitively,
    // via tests/unit/dbt/test_address.py) the dbt macro.
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("address_golden.json")).unwrap();
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let cases = golden["cases"].as_array().unwrap();
    let count = cases.len();
    assert!(count > 20, "expected a broad case set");

    for case in cases {
        let input = case["input"].as_str();
        let projection = address_parse("col", "null", "null", "null");
        let sql = format!(
            "select {projection} from (select cast({} as varchar) as col) as input",
            lit(input)
        );
        let rows = session.query_strings(&sql).unwrap();
        assert_eq!(
            rows[0][0],
            expected_text(&case["addr_number"]),
            "addr_number for {input:?} differs",
        );
        assert_eq!(
            rows[0][1],
            expected_text(&case["addr_street"]),
            "addr_street for {input:?} differs",
        );
        assert_eq!(
            rows[0][2],
            expected_text(&case["addr_unit"]),
            "addr_unit for {input:?} differs",
        );
    }
}
