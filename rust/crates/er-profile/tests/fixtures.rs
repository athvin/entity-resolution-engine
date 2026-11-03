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

/// A measured candidate key, for the corpus rules.
fn candidate(
    name: &str,
    rows: i64,
    covered: i64,
    expected_pairs: i64,
) -> dedupe_core::blocking_analysis::CandidateKeyEstimate {
    dedupe_core::blocking_analysis::CandidateKeyEstimate {
        key_columns: vec![name.to_string()],
        rows,
        covered_rows: covered,
        distinct_values: 1000,
        expected_pairs,
        largest_block_pairs: 10,
        largest_block_rows: 5,
        entropy_bits: 9.0,
        pair_loss_from_nulls: 1.0 - (covered as f64 / rows as f64).powi(2),
    }
}

/// Codes the registry reports for a corpus with these candidate keys.
fn corpus_codes(
    candidates: &[dedupe_core::blocking_analysis::CandidateKeyEstimate],
) -> Vec<&'static str> {
    er_profile::findings_with(&[(healthy("email"), vec![])], candidates)
        .into_iter()
        .map(|f| f.code.0)
        .collect()
}

#[test]
fn p009_fires_only_when_no_key_is_viable() {
    // 100k rows: C(n,2) is ~5e9, so 1e6 pairs eliminates >99.9% — viable.
    let good = candidate("email", 100_000, 95_000, 1_000_000);
    assert!(!corpus_codes(&[good]).contains(&"P009"));

    // The same corpus where the best key still leaves 14% of all comparisons.
    let bad = candidate("addr_city", 100_000, 95_000, 700_000_000);
    let reported = corpus_codes(&[bad]);
    assert!(reported.contains(&"P009"), "got {reported:?}");
}

#[test]
fn p009_counts_a_cheap_but_empty_key_as_not_viable() {
    // THE trap this rule exists for: a key can be cheap because its column is
    // almost entirely NULL, and recommending it would recommend a key that
    // cannot see the data. 5% covered => 99.75% of pairs unreachable.
    let cheap_but_blind = candidate("fax", 100_000, 5_000, 1_000);
    let reported = corpus_codes(&[cheap_but_blind]);
    assert!(
        reported.contains(&"P009"),
        "a key covering 5% of rows is not viable however cheap it looks, got {reported:?}"
    );
    assert!(
        !reported.contains(&"P010"),
        "and it must not be recommended either, got {reported:?}"
    );
}

#[test]
fn p010_names_the_cheapest_viable_key_not_the_cheapest_overall() {
    let viable = candidate("email", 100_000, 95_000, 1_000_000);
    let cheaper_but_blind = candidate("fax", 100_000, 4_000, 500);
    let found =
        er_profile::findings_with(&[(healthy("email"), vec![])], &[cheaper_but_blind, viable]);
    let p010 = found
        .iter()
        .find(|f| f.code == Code("P010"))
        .expect("P010 should name a key");
    assert_eq!(
        p010.detail["key"][0], "email",
        "the cheapest key overall is `fax`, but it is unusable; P010 must name `email`"
    );
    assert_eq!(p010.severity, Severity::Info);
}

#[test]
fn the_corpus_rules_stay_silent_when_nothing_was_measured() {
    // An empty candidate list means "tier 2 did not run", NOT "no key is
    // viable". Reporting P009 here would invent a finding from an absence.
    let reported = corpus_codes(&[]);
    assert!(!reported.contains(&"P009"), "got {reported:?}");
    assert!(!reported.contains(&"P010"), "got {reported:?}");
}

#[test]
fn the_corpus_rules_stay_silent_on_a_corpus_too_small_to_block() {
    // Below ~1000 rows the cartesian product is cheap and "no good key exists"
    // is true but useless. A warning on every small dataset teaches people to
    // ignore the panel.
    let reported = corpus_codes(&[candidate("addr_city", 200, 200, 19_900)]);
    assert!(!reported.contains(&"P009"), "got {reported:?}");
    assert!(!reported.contains(&"P010"), "got {reported:?}");
}

#[test]
fn severity_grades_wrong_results_apart_from_costs() {
    // The scale has three levels and must use all three, or it has two levels
    // and a dead branch. `error` is reserved for a WRONG RESULT; everything
    // that merely costs recall, runtime or trust is a `warning`.
    //
    // P004 (placeholders) and P008 (a colliding identifier) are the two that
    // corrupt output rather than degrade it: the first merges records that
    // share nothing but their missing data, the second loses a row to whichever
    // copy landed last. Both are silent in the result.
    let by_code: std::collections::BTreeMap<&str, Severity> = er_profile::rule_infos()
        .iter()
        .map(|i| (i.code.0, i.severity))
        .collect();

    assert_eq!(
        by_code["P004"],
        Severity::Error,
        "placeholders cause false merges"
    );
    assert_eq!(by_code["P008"], Severity::Error, "id collisions lose rows");

    // P009 is deliberately NOT an error: an unblockable corpus costs runtime or
    // recall, which is exactly the `warning` definition. It does not corrupt.
    assert_eq!(by_code["P009"], Severity::Warning);

    // All three levels are in use.
    let used: std::collections::BTreeSet<&str> = by_code.values().map(|s| s.as_str()).collect();
    assert_eq!(
        used,
        ["error", "info", "warning"].into_iter().collect(),
        "every severity level must have at least one rule, or the scale is decorative"
    );
}

#[test]
fn the_blocking_rules_gate_on_pairs_not_rows() {
    // The threshold is about the quadratic blocking defuses, so it is measured
    // in pairs. 3000 rows is 4.5M pairs — under budget, silent. 4000 rows is
    // 8M — over, so the rules speak. A row-count floor could not express that.
    let under = candidate("addr_city", 3_000, 3_000, 4_000_000);
    assert!(
        !corpus_codes(&[under]).contains(&"P009"),
        "4.5M unblocked pairs is still cheap"
    );
    let over = candidate("addr_city", 4_000, 4_000, 7_000_000);
    assert!(
        corpus_codes(&[over]).contains(&"P009"),
        "8M unblocked pairs is worth a sentence"
    );
}

// ---------------------------------------------------------------------------
// Drift (P011, P012) — the only rules that compare two profiles.
// ---------------------------------------------------------------------------

/// A previous column, given a fill rate expressed as present-of-1000.
fn was(name: &str, present: i64) -> er_profile::PreviousColumn {
    er_profile::PreviousColumn {
        column_name: name.to_string(),
        row_count: 1000,
        non_null_count: present,
        blank_count: 0,
    }
}

fn previous(columns: Vec<er_profile::PreviousColumn>) -> er_profile::PreviousProfile {
    er_profile::PreviousProfile {
        captured_at: "2026-01-01 00:00:00".to_string(),
        columns,
    }
}

/// Run the registry over one column against a previous profile.
fn drift_codes(
    shape: ColumnShape,
    prev: Option<&er_profile::PreviousProfile>,
) -> Vec<&'static str> {
    er_profile::findings_against(&[(shape, vec![])], &[], prev)
        .into_iter()
        .map(|f| f.code.0)
        .collect()
}

#[test]
fn p011_fires_when_a_fill_drop_costs_real_reach() {
    // `healthy` is 940/1000 present. Against a previous 950/1000 that is a one
    // point move, costing ~2% of reachable pairs — inside the noise a
    // delivery's composition creates, and silent.
    let quiet = previous(vec![was("email", 950)]);
    assert!(!drift_codes(healthy("email"), Some(&quiet)).contains(&"P011"));

    // Against a previous 1000/1000, 940 is a 12% pair loss: 1 - 0.94² = 0.116.
    let noisy = previous(vec![was("email", 1000)]);
    let reported = drift_codes(healthy("email"), Some(&noisy));
    assert!(reported.contains(&"P011"), "got {reported:?}");
}

#[test]
fn p011_is_silent_without_a_previous_profile() {
    // The first profile of a corpus has nothing to compare against, and "has it
    // changed" has no answer. The bug this guards is reading None as "no
    // change" OR as "everything appeared" — both are answers to a question
    // nobody could ask yet.
    let reported = drift_codes(healthy("email"), None);
    assert!(!reported.contains(&"P011"), "got {reported:?}");
    assert!(!reported.contains(&"P012"), "got {reported:?}");
}

#[test]
fn p011_ignores_a_column_that_was_already_mostly_empty() {
    // 100/1000 -> 20/1000 is a 96% pair loss, and reporting it would be the
    // same complaint P002 already makes about a column nobody can match on.
    let mut sparse = healthy("fax");
    sparse.non_null_count = 20;
    let before = previous(vec![was("fax", 100)]);
    let reported = drift_codes(sparse, Some(&before));
    assert!(
        !reported.contains(&"P011"),
        "below 20% previous fill this is P002's business, got {reported:?}"
    );
}

#[test]
fn p011_does_not_complain_about_a_column_filling_up() {
    // A column going from 500 to 940 present is good news; a warning here
    // would teach people that drift findings are noise.
    let before = previous(vec![was("email", 500)]);
    let reported = drift_codes(healthy("email"), Some(&before));
    assert!(!reported.contains(&"P011"), "got {reported:?}");
}

#[test]
fn p012_reports_both_directions_with_the_right_severity() {
    // `phone_e164` was there and is gone; `nickname` is new. One rule, because
    // "did the column set change" is one question and a reader comparing two
    // deliveries wants both halves of the answer.
    let before = previous(vec![was("email", 940), was("phone_e164", 900)]);
    let found = er_profile::findings_against(
        &[(healthy("email"), vec![]), (healthy("nickname"), vec![])],
        &[],
        Some(&before),
    );

    let gone = found
        .iter()
        .find(|f| f.code == Code("P012") && f.column_name == "phone_e164")
        .expect("the disappearance is reported");
    assert_eq!(
        gone.severity,
        Severity::Warning,
        "a column vanishing changes what can be matched"
    );
    assert!(gone.message.contains("absent from this one"));

    let arrived = found
        .iter()
        .find(|f| f.code == Code("P012") && f.column_name == "nickname")
        .expect("the arrival is reported");
    assert_eq!(
        arrived.severity,
        Severity::Info,
        "a new column is not a problem, just worth mapping"
    );

    // Unchanged columns produce nothing — the rule reports CHANGES, and a
    // finding per steady column would bury the two that matter.
    assert_eq!(
        found.iter().filter(|f| f.code == Code("P012")).count(),
        2,
        "only the two changes, got {found:?}"
    );
}

#[test]
fn p012_is_quiet_when_the_column_set_is_unchanged() {
    let before = previous(vec![was("email", 940)]);
    let reported = drift_codes(healthy("email"), Some(&before));
    assert!(!reported.contains(&"P012"), "got {reported:?}");
}

// ---------------------------------------------------------------------------
// Message safety: what a finding's prose may contain.
// ---------------------------------------------------------------------------

/// A finding's `message` may name a value ONLY from the closed placeholder set.
///
/// This is load-bearing outside the registry. The profile's AI narrative sends
/// findings to a third-party model, and it sends the prose because a code alone
/// ("P004 on email") is not enough for the model to say anything useful. That is
/// only safe while the single rule that interpolates a raw value — P004 — draws
/// it from a hardcoded list of twelve absence markers rather than matching a
/// pattern. The plan for this rule originally proposed `test@*`; had that
/// shipped, the message would carry a real address and the narrative would ship
/// customer data to answer a question about data quality.
///
/// So the property is asserted rather than trusted: given a column whose top
/// values include both a real-looking address and a placeholder, the finding
/// must name the placeholder and must not name the address.
#[test]
fn a_finding_message_names_placeholders_and_never_real_values() {
    let mut column = healthy("email");
    column.row_count = 1000;
    let top = vec![
        freq("email", "jane.doe@acme.com", 400, 0.40, 1),
        freq("email", "n/a", 120, 0.12, 2),
    ];

    let found = findings(&[(column, top)]);
    assert!(
        !found.is_empty(),
        "the fixture should trip at least the placeholder rule"
    );
    for finding in &found {
        assert!(
            !finding.message.contains("jane.doe@acme.com"),
            "{} put a real value in its message: {}",
            finding.code.0,
            finding.message
        );
    }
    let placeholder = found
        .iter()
        .find(|f| f.code == Code("P004"))
        .expect("P004 should fire on a 12% placeholder share");
    assert!(
        placeholder.message.contains("n/a"),
        "P004 must name WHICH placeholder, or the reader cannot find it: {}",
        placeholder.message
    );
}
