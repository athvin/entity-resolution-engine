//! One conforming and one violating case per rule, built from literals.
//!
//! The registry judges a computed profile and nothing else — no database, no
//! config, no model — which is exactly what makes a literal fixture possible,
//! and the reason rules needing the tenant's config are deliberately not here.
//!
//! Each case asserts the rule fires on the violating shape AND stays quiet on
//! the conforming one. Only asserting the first would let a rule that returns a
//! finding unconditionally pass every test it has.

use dedupe_core::profile::{ColumnShape, ValueFrequency};
use er_profile::{Code, Severity, findings};

/// A fully populated, well-behaved text column: 1000 rows, 940 present, high
/// cardinality, nothing parsing as another type.
fn healthy(name: &str) -> ColumnShape {
    ColumnShape {
        column_name: name.to_string(),
        row_count: 1000,
        total_row_count: 1000,
        non_null_count: 940,
        blank_count: 0,
        distinct_approx: 900,
        distinct_exact: Some(900),
        distinct_normalized: Some(900),
        len_min: Some(3),
        len_max: Some(20),
        len_avg: Some(8.0),
        len_p50: Some(8),
        len_p95: Some(15),
        lex_min: Some("aaa".to_string()),
        lex_max: Some("zzz".to_string()),
        num_min: None,
        num_max: None,
        num_avg: None,
        num_std: None,
        cast_bigint: 0,
        cast_double: 0,
        cast_date: 0,
        cast_timestamp: 0,
        cast_boolean: 0,
        entropy_bits: Some(9.5),
    }
}

fn freq(column: &str, value: &str, count: i64, share: f64, rank: i64) -> ValueFrequency {
    ValueFrequency {
        column_name: column.to_string(),
        value: Some(value.to_string()),
        count,
        share,
        rank,
    }
}

/// Run the registry over one column and return the codes it reported.
fn codes_for(shape: ColumnShape, top: Vec<ValueFrequency>) -> Vec<&'static str> {
    findings(&[(shape, top)])
        .into_iter()
        .map(|f| f.code.0)
        .collect()
}

/// Assert `code` fires on `violating` and is silent on `conforming`.
fn assert_rule(
    code: &str,
    conforming: (ColumnShape, Vec<ValueFrequency>),
    violating: (ColumnShape, Vec<ValueFrequency>),
) {
    let quiet = codes_for(conforming.0, conforming.1);
    assert!(
        !quiet.contains(&code),
        "{code} fired on its own conforming example (reported {quiet:?})"
    );
    let loud = codes_for(violating.0, violating.1);
    assert!(
        loud.contains(&code),
        "{code} did not fire on its own violating example (reported {loud:?})"
    );
}

#[test]
fn p001_column_is_empty() {
    let mut empty = healthy("middle_name");
    empty.non_null_count = 0;
    empty.blank_count = 0;
    empty.distinct_exact = Some(0);
    empty.distinct_normalized = Some(0);
    assert_rule("P001", (healthy("email"), vec![]), (empty, vec![]));
}

#[test]
fn p001_counts_a_blank_only_column_as_empty() {
    // The repo-specific case: ingest lands every value as a JSON string, so an
    // empty cell arrives as '' and is NOT NULL. A NULL-only check would call
    // this column fully populated.
    let mut blanks = healthy("middle_name");
    blanks.non_null_count = 1000;
    blanks.blank_count = 1000;
    let reported = codes_for(blanks, vec![]);
    assert!(
        reported.contains(&"P001"),
        "a column of 1000 blank-but-present values is empty, not full (got {reported:?})"
    );
}

#[test]
fn p002_column_is_mostly_empty() {
    let mut sparse = healthy("addr_postal");
    sparse.non_null_count = 410;
    assert_rule("P002", (healthy("phone_e164"), vec![]), (sparse, vec![]));
}

#[test]
fn p002_defers_to_p001_on_a_fully_empty_column() {
    // Both rules are about absence; the more specific one should be the only
    // one talking, or the actionable finding gets buried under the vaguer one.
    let mut empty = healthy("middle_name");
    empty.non_null_count = 0;
    let reported = codes_for(empty, vec![]);
    assert!(reported.contains(&"P001"));
    assert!(
        !reported.contains(&"P002"),
        "P002 should stay silent when P001 already said it better (got {reported:?})"
    );
}

#[test]
fn p003_one_value_dominates() {
    let city = healthy("addr_city");
    assert_rule(
        "P003",
        (
            healthy("surname"),
            vec![freq("surname", "Jones", 23, 0.028, 1)],
        ),
        (city, vec![freq("addr_city", "London", 310, 0.31, 1)]),
    );
}

#[test]
fn p003_stays_silent_when_no_values_were_counted() {
    // top_values is a second pass. A share inferred from the shape alone would
    // be invented, so the rule must decline rather than guess.
    let reported = codes_for(healthy("addr_city"), vec![]);
    assert!(
        !reported.contains(&"P003"),
        "P003 must not fire without counted values (got {reported:?})"
    );
}

#[test]
fn p004_placeholder_values_present() {
    assert_rule(
        "P004",
        (
            healthy("addr_region"),
            vec![freq("addr_region", "CA", 120, 0.12, 1)],
        ),
        (
            healthy("phone"),
            vec![freq("phone", "unknown", 40, 0.04, 1)],
        ),
    );
}

#[test]
fn p004_matches_a_placeholder_whatever_its_case_or_padding() {
    for spelling in ["N/A", " n/a ", "NULL", "None", "--"] {
        let reported = codes_for(healthy("phone"), vec![freq("phone", spelling, 40, 0.04, 1)]);
        assert!(
            reported.contains(&"P004"),
            "{spelling:?} should be recognised as a placeholder (got {reported:?})"
        );
    }
}

#[test]
fn p004_does_not_flag_a_real_value_that_merely_looks_like_one() {
    // `none` is a surname and `null` a place name; the list is short on purpose,
    // but a value below the share threshold must not fire either way.
    let reported = codes_for(
        healthy("surname"),
        vec![freq("surname", "None", 2, 0.002, 1)],
    );
    assert!(
        !reported.contains(&"P004"),
        "a placeholder below the share threshold must not fire (got {reported:?})"
    );
}

#[test]
fn p005_normalization_collapses_values() {
    let mut messy = healthy("addr_city");
    messy.distinct_exact = Some(216);
    messy.distinct_normalized = Some(180);
    assert_rule("P005", (healthy("email"), vec![]), (messy, vec![]));
}

#[test]
fn p005_stays_silent_without_the_exact_counts() {
    // Both counts are the expensive exact ones. Without them there is nothing
    // to compare, and the sketch is far too noisy to stand in.
    let mut no_exact = healthy("addr_city");
    no_exact.distinct_exact = None;
    no_exact.distinct_normalized = None;
    let reported = codes_for(no_exact, vec![]);
    assert!(
        !reported.contains(&"P005"),
        "P005 must not fire from the approximate count (got {reported:?})"
    );
}

#[test]
fn p006_text_holds_a_consistent_type() {
    let mut dates = healthy("dob");
    dates.cast_date = 940;
    dates.cast_timestamp = 940;
    assert_rule("P006", (healthy("given_name"), vec![]), (dates, vec![]));
}

#[test]
fn p006_reports_the_narrowest_matching_type() {
    // A date parses as a timestamp too, and an integer as a double. Reporting
    // the widest match would be true and useless.
    let mut dates = healthy("dob");
    dates.cast_date = 940;
    dates.cast_timestamp = 940;
    let found = findings(&[(dates, vec![])]);
    let p006 = found.iter().find(|f| f.code == Code("P006")).expect("P006");
    assert_eq!(p006.detail["inferred_type"], "date");
    assert_eq!(p006.severity, Severity::Info, "a text date may be correct");
}

#[test]
fn p007_column_is_constant() {
    let mut constant = healthy("country");
    constant.distinct_exact = Some(1);
    constant.distinct_normalized = Some(1);
    assert_rule(
        "P007",
        (healthy("source_system"), vec![]),
        (constant, vec![]),
    );
}

#[test]
fn p008_identifier_is_not_unique() {
    let mut colliding = healthy("crm_id");
    colliding.non_null_count = 1000;
    colliding.distinct_exact = Some(997);
    colliding.distinct_normalized = Some(997);

    let mut unique = healthy("crm_id");
    unique.non_null_count = 1000;
    unique.distinct_exact = Some(1000);
    unique.distinct_normalized = Some(1000);

    assert_rule("P008", (unique, vec![]), (colliding, vec![]));
}

#[test]
fn p008_refuses_to_answer_from_the_sketch() {
    // The sketch is wrong in both directions (978 measured on 1000 genuinely
    // distinct ids), so inferring uniqueness from it would invent collisions
    // AND miss real ones. `is_unique` is None without the exact count.
    let mut sketch_only = healthy("crm_id");
    sketch_only.non_null_count = 1000;
    sketch_only.distinct_approx = 978;
    sketch_only.distinct_exact = None;
    sketch_only.distinct_normalized = None;
    let reported = codes_for(sketch_only, vec![]);
    assert!(
        !reported.contains(&"P008"),
        "P008 must not fire from the approximate count (got {reported:?})"
    );
}

#[test]
fn p008_ignores_a_column_nobody_expected_to_be_unique() {
    // 900 distinct over 940 rows is a surname column, not a broken identifier.
    let reported = codes_for(healthy("surname"), vec![]);
    assert!(
        !reported.contains(&"P008"),
        "P008 should only speak about near-unique columns (got {reported:?})"
    );
}

#[test]
fn a_healthy_column_reports_nothing_at_all() {
    // The whole-registry version of the conforming case: if any rule fires on
    // a clean column, every profile will be noisy and the findings panel stops
    // being read.
    let reported = codes_for(
        healthy("email"),
        vec![freq("email", "a@b.com", 3, 0.003, 1)],
    );
    assert!(
        reported.is_empty(),
        "a healthy column must produce no findings, got {reported:?}"
    );
}
