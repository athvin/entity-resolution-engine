//! PR 1.6 Bucket A: the new single-winner survivorship rule tokens elect the
//! expected record. Runs the generated `survivorship_decision_sql` over an
//! in-memory DuckDB session with a tiny candidate table. These rules reference
//! only `value`/`entity_id`/`record_key`, so no sources/validity columns are
//! needed. (End-to-end over the real lake for the existing rules is gated by
//! er-match's base_10 golden test, which these additive tokens leave untouched.)

use dedupe_core::session::{Session, SessionOptions};
use er_golden::survivorship_decision_sql;

/// Elect the rank-1 winner for `chain` over `rows` (entity_id, value, record_key).
/// Returns (value, record_key, rule).
fn elect(rows: &[(&str, &str, &str)], chain: &[&str]) -> (String, String, String) {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    session
        .execute_batch(
            "CREATE OR REPLACE TABLE cand(entity_id VARCHAR, value VARCHAR, record_key VARCHAR)",
        )
        .unwrap();
    let values = rows
        .iter()
        .map(|(e, v, k)| format!("('{e}', '{v}', '{k}')"))
        .collect::<Vec<_>>()
        .join(", ");
    session
        .execute_batch(&format!("INSERT INTO cand VALUES {values}"))
        .unwrap();
    let chain: Vec<String> = chain.iter().map(|s| (*s).to_string()).collect();
    let sql = survivorship_decision_sql("value", &chain, "cand").unwrap();
    let out = session.query_strings(&sql).unwrap();
    assert_eq!(out.len(), 1, "one winner row per entity");
    let r = &out[0];
    (r[1].clone(), r[2].clone(), r[3].clone())
}

#[test]
fn longest_elects_the_longest_value() {
    let (value, key, rule) = elect(
        &[
            ("e1", "ab", "r1"),
            ("e1", "abcd", "r2"),
            ("e1", "abc", "r3"),
        ],
        &["longest", "record_key ASC"],
    );
    assert_eq!(
        (value.as_str(), key.as_str(), rule.as_str()),
        ("abcd", "r2", "longest")
    );
}

#[test]
fn shortest_elects_the_shortest_value() {
    let (_, key, rule) = elect(
        &[
            ("e1", "ab", "r1"),
            ("e1", "abcd", "r2"),
            ("e1", "abc", "r3"),
        ],
        &["shortest", "record_key ASC"],
    );
    assert_eq!((key.as_str(), rule.as_str()), ("r1", "shortest"));
}

#[test]
fn numeric_max_and_min_cast_before_comparing() {
    // String order would put '5' > '20'; numeric must pick 20 / 3.
    let rows = [("e1", "5", "r1"), ("e1", "20", "r2"), ("e1", "3", "r3")];
    let (vmax, kmax, rmax) = elect(&rows, &["numeric_max", "record_key ASC"]);
    assert_eq!(
        (vmax.as_str(), kmax.as_str(), rmax.as_str()),
        ("20", "r2", "numeric_max")
    );
    let (vmin, kmin, rmin) = elect(&rows, &["numeric_min", "record_key ASC"]);
    assert_eq!(
        (vmin.as_str(), kmin.as_str(), rmin.as_str()),
        ("3", "r3", "numeric_min")
    );
}

#[test]
fn most_common_elects_the_modal_value() {
    let (value, _, _) = elect(
        &[("e1", "x", "r1"), ("e1", "x", "r2"), ("e1", "y", "r3")],
        &["most_common", "record_key ASC"],
    );
    assert_eq!(value, "x", "the value held by the most member records wins");
}

#[test]
fn prioritized_ranks_by_the_configured_list() {
    // Ranked b > a > c; unlisted sorts last.
    let (value, key, rule) = elect(
        &[
            ("e1", "a", "r1"),
            ("e1", "b", "r2"),
            ("e1", "c", "r3"),
            ("e1", "z", "r4"),
        ],
        &["prioritized:b|a|c", "record_key ASC"],
    );
    assert_eq!(
        (value.as_str(), key.as_str(), rule.as_str()),
        ("b", "r2", "prioritized:b|a|c")
    );
}

#[test]
fn prioritized_falls_through_when_no_value_is_listed() {
    // None listed -> all tie on the rule -> terminal record_key ASC decides.
    let (_, key, rule) = elect(
        &[("e1", "p", "r2"), ("e1", "q", "r1")],
        &["prioritized:x|y", "record_key ASC"],
    );
    assert_eq!(
        (key.as_str(), rule.as_str()),
        ("r1", "tiebreak_deterministic")
    );
}

// --- PR 1.6b (Bucket B): value-synthesizing rules. The decision's `value` column
// IS the synthesized value (combined_list / sum); `record_key` is the
// deterministic representative (`min`) so lineage still names a member, and the
// `rule` is the synthesizing rule. (Record-assembly over the real lake reuses the
// same aggregate via `synthesized_value_expr`.) ---

#[test]
fn combined_list_unions_distinct_sorted_values() {
    // Duplicates collapse, blanks drop, output is sorted and '; '-joined.
    let (value, key, rule) = elect(
        &[
            ("e1", "b", "r2"),
            ("e1", "a", "r1"),
            ("e1", "b", "r3"),
            ("e1", "", "r4"),
        ],
        &["combined_list"],
    );
    assert_eq!(
        (value.as_str(), key.as_str(), rule.as_str()),
        ("a; b", "r1", "combined_list")
    );
}

#[test]
fn sum_totals_numeric_values_dropping_non_numeric() {
    // 10 + 5 = 15; "x" casts to NULL and drops out; representative is min key.
    let (value, key, rule) = elect(
        &[("e1", "10", "r1"), ("e1", "5", "r2"), ("e1", "x", "r3")],
        &["sum"],
    );
    assert_eq!(
        (value.as_str(), key.as_str(), rule.as_str()),
        ("15.0", "r1", "sum")
    );
}

#[test]
fn a_synthesizing_rule_cannot_be_mixed_with_ranking_rules() {
    let chain = ["combined_list".to_string(), "longest".to_string()];
    let err = survivorship_decision_sql("value", &chain, "cand").unwrap_err();
    assert!(
        err.to_string().contains("only"),
        "expected a mixing error, got: {err}"
    );
}

#[test]
fn a_synthesizing_rule_is_rejected_on_the_composite_address() {
    let chain = ["combined_list".to_string()];
    let err = survivorship_decision_sql("address", &chain, "cand").unwrap_err();
    assert!(
        err.to_string().contains("address"),
        "expected an address error, got: {err}"
    );
}
