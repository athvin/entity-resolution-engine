//! Profiling gates: the per-column shape is deterministic, and the model-free
//! candidate-key estimate agrees with the exact blocking count.
//!
//! The second of those is the load-bearing one. `expected_pairs` exists so that
//! blocking can be costed on a corpus that has never been trained, which means
//! nothing downstream can check it against a model — so it is checked here
//! against `count_comparisons_from_blocking_rules`, which does use one. If the
//! two ever disagree, the cheap path is lying.

use crate::common;

use common::{FAKE_1000_SCHEMA, fake_1000_csv_path, load_golden_json, load_golden_text};
use dedupe_core::Settings;
use dedupe_core::blocking_analysis::{
    count_comparisons_from_blocking_rules, estimate_candidate_key, n_largest_blocks,
    rank_candidate_keys,
};
use dedupe_core::predict;
use dedupe_core::profile::{
    ColumnShape, ProfileOptions, ProfileSample, column_shape, modes, read_column_shapes, top_values,
};
use dedupe_core::session::{Session, SessionOptions, TableHandle};

/// The columns fake_1000 carries, as the golden generator ingests them.
const PROFILED: [&str; 5] = ["first_name", "surname", "dob", "city", "email"];

fn fixture() -> (Session, Settings, TableHandle) {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = session
        .register_csv(&fake_1000_csv_path(), "fake_1000", FAKE_1000_SCHEMA)
        .unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();
    let concat = predict::materialize_concat(&session, &input).unwrap();
    (session, settings, concat)
}

fn profile(session: &Session, concat: &TableHandle, options: &ProfileOptions) -> Vec<ColumnShape> {
    let columns: Vec<String> = PROFILED.iter().map(|c| (*c).to_string()).collect();
    let shape = column_shape(session, concat, &columns, options).unwrap();
    let rows = read_column_shapes(session, &shape).unwrap();
    session.drop_table(&shape).unwrap();
    rows
}

fn by_name<'a>(rows: &'a [ColumnShape], name: &str) -> &'a ColumnShape {
    rows.iter()
        .find(|r| r.column_name == name)
        .unwrap_or_else(|| panic!("no shape row for {name}"))
}

#[test]
fn column_shape_is_deterministic_and_matches_measured_values() {
    let (session, _settings, concat) = fixture();
    let options = ProfileOptions {
        exact_distinct: true,
        sample: None,
    };

    let first = profile(&session, &concat, &options);
    let second = profile(&session, &concat, &options);
    assert_eq!(
        first, second,
        "two profiles of one corpus must be identical"
    );

    assert_eq!(first.len(), PROFILED.len());

    // Independently measured against the fixture CSV.
    let surname = by_name(&first, "surname");
    assert_eq!(surname.row_count, 1000);
    assert_eq!(surname.total_row_count, 1000);
    assert_eq!(surname.non_null_count, 819);
    assert_eq!(surname.distinct_exact, Some(371));
    assert_eq!(
        surname.entropy_bits.map(|e| (e * 1e4).round() / 1e4),
        Some(7.9676),
        "entropy must match DuckDB's own entropy() over the same column"
    );

    // `dob` is declared VARCHAR, and every value parses as a date. The declared
    // type cannot say that; conformance is the only thing that can.
    let dob = by_name(&first, "dob");
    assert_eq!(dob.non_null_count, 1000);
    assert_eq!(dob.cast_date, 1000, "every dob parses as a date");
    assert_eq!(dob.cast_bigint, 0, "a dashed date is not an integer");

    // Nothing in this fixture is blank-but-present, so the metric must report
    // exactly zero rather than conflating blanks with the 181 NULLs.
    assert_eq!(surname.blank_count, 0);
    assert_eq!(
        surname.non_null_count - surname.blank_count,
        819,
        "blank and null are counted separately"
    );

    for row in &first {
        let ratio = row.cardinality_ratio().expect("non-empty column");
        assert!(
            (0.0..=1.0).contains(&ratio),
            "{}: cardinality_ratio {ratio} escaped [0,1] — the HLL sketch \
             overshoots and must be clamped",
            row.column_name
        );
    }
}

#[test]
fn a_sampled_profile_is_a_stable_subset() {
    let (session, _settings, concat) = fixture();
    let options = ProfileOptions {
        exact_distinct: true,
        sample: Some(ProfileSample {
            id_expr: "unique_id".to_string(),
            proportion: 0.25,
            seed: Some(20_260_101),
        }),
    };

    let first = profile(&session, &concat, &options);
    let second = profile(&session, &concat, &options);
    assert_eq!(
        first, second,
        "a SAMPLED profile must also be reproducible, or no two profiles are comparable"
    );

    let surname = by_name(&first, "surname");
    assert!(
        surname.row_count > 0 && surname.row_count < surname.total_row_count,
        "expected a proper subset, read {} of {}",
        surname.row_count,
        surname.total_row_count
    );
    assert_eq!(
        surname.total_row_count, 1000,
        "the corpus size must be reported unsampled"
    );

    // A different seed over the same corpus must draw a different sample,
    // otherwise the seed is not wired through and "deterministic" would be
    // indistinguishable from "constant".
    let other = profile(
        &session,
        &concat,
        &ProfileOptions {
            exact_distinct: true,
            sample: Some(ProfileSample {
                id_expr: "unique_id".to_string(),
                proportion: 0.25,
                seed: Some(7),
            }),
        },
    );
    assert_ne!(
        by_name(&other, "surname").row_count,
        surname.row_count,
        "a different seed must select a different sample"
    );
}

#[test]
fn expected_pairs_equals_the_exact_blocking_count() {
    let (session, settings, concat) = fixture();

    // Rule 0 of the committed blocking-analysis golden. Being FIRST, it has no
    // preceding rule to dedup against, so its marginal count is its standalone
    // count — which is what the model-free estimate must reproduce.
    let golden = load_golden_json("blocking_analysis.json");
    let expected_rule_0 = golden.as_array().unwrap()[0]["marginal_comparison_count"]
        .as_i64()
        .unwrap();

    let key = vec!["first_name".to_string(), "city".to_string()];
    let estimate = estimate_candidate_key(&session, &concat, &key).unwrap();
    assert_eq!(
        estimate.expected_pairs, expected_rule_0,
        "the model-free estimate must equal the committed exact count"
    );

    // And against the counter itself, through the model, in this same session.
    let counts = count_comparisons_from_blocking_rules(
        &session,
        &settings,
        &concat,
        &[r#"(l."first_name" = r."first_name") AND (l."city" = r."city")"#.to_string()],
    )
    .unwrap();
    assert_eq!(
        estimate.expected_pairs, counts[0].marginal_comparison_count,
        "the two paths must agree pair for pair"
    );
    assert_eq!(estimate.rows, 1000);
    assert_eq!(
        counts[0].total_possible_comparison_count, 499_500,
        "C(1000,2)"
    );
}

#[test]
fn a_candidate_key_reports_its_skew_and_its_null_cost() {
    let (session, _settings, concat) = fixture();

    let surname = estimate_candidate_key(&session, &concat, &["surname".to_string()]).unwrap();

    // Measured directly against the fixture.
    assert_eq!(surname.expected_pairs, 1638);
    assert_eq!(surname.distinct_values, 371);
    assert_eq!(surname.covered_rows, 819);
    assert_eq!(
        surname.largest_block_pairs, 253,
        "one surname accounts for 253 of the 1638 pairs"
    );

    // 819 of 1000 rows carry a surname, so (819/1000)^2 of all pairs are
    // reachable and the rest are lost to NULLs on one side or the other.
    let expected_loss = 1.0 - (819.0_f64 / 1000.0).powi(2);
    assert!(
        (surname.pair_loss_from_nulls - expected_loss).abs() < 1e-12,
        "pair_loss_from_nulls {} != {expected_loss}",
        surname.pair_loss_from_nulls
    );
    assert!(
        surname.entropy_bits > 7.0,
        "surname entropy {} should be high relative to city",
        surname.entropy_bits
    );

    // city is the less discriminating key and must rank as the more expensive
    // one; the ranking is by pair count ascending.
    let ranked = rank_candidate_keys(
        &session,
        &concat,
        &[vec!["city".to_string()], vec!["surname".to_string()]],
    )
    .unwrap();
    assert_eq!(ranked[0].key_columns, vec!["surname".to_string()]);
    assert!(
        ranked[0].expected_pairs < ranked[1].expected_pairs,
        "surname ({}) should be cheaper than city ({})",
        ranked[0].expected_pairs,
        ranked[1].expected_pairs
    );
    assert!(
        ranked[1].entropy_bits < ranked[0].entropy_bits,
        "the more expensive key should carry less information per value"
    );
}

#[test]
fn a_filter_rule_has_no_block_breakdown_and_says_so() {
    let (session, settings, concat) = fixture();

    let equi = n_largest_blocks(
        &session,
        &settings,
        &concat,
        r#"l."first_name" = r."first_name""#,
        3,
    )
    .unwrap();
    assert!(
        equi.is_some_and(|b| !b.is_empty()),
        "a pure equi-join has a breakdown"
    );

    // Not a pure equi-join. The honest answer is "cannot be broken down",
    // which must be distinguishable from "produces no large blocks" — an empty
    // list here would tell a caller the rule is safe when nothing was measured.
    let filtered = n_largest_blocks(
        &session,
        &settings,
        &concat,
        r#"levenshtein(l."surname", r."surname") < 3"#,
        3,
    )
    .unwrap();
    assert!(
        filtered.is_none(),
        "a filter rule must report None, not an empty list"
    );
}

#[test]
fn largest_blocks_quotes_embedded_quotes_in_key_names() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    session
        .execute_batch(
            r#"CREATE TEMP TABLE quoted ("a""b" VARCHAR);
               INSERT INTO quoted VALUES ('x'), ('x'), ('y');"#,
        )
        .unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();
    let concat = TableHandle {
        templated: "quoted".into(),
        physical: "quoted".into(),
        columns: vec!["a\"b".into()],
    };
    let blocks = n_largest_blocks(&session, &settings, &concat, r#"l."a""b" = r."a""b""#, 2)
        .unwrap()
        .unwrap();
    assert_eq!(blocks[0].key_values, vec!["x"]);
    assert_eq!((blocks[0].count_l, blocks[0].count_r), (2, 2));
    assert_eq!(blocks[0].block_count, 4);
}

#[test]
fn the_dedupe_block_pair_count_reconciles_with_the_pair_total() {
    let (session, settings, concat) = fixture();

    let blocks = n_largest_blocks(
        &session,
        &settings,
        &concat,
        r#"l."surname" = r."surname""#,
        1,
    )
    .unwrap()
    .expect("equi-join");
    let largest = &blocks[0];

    // `block_count` is count_l * count_r — ordered pairs including self-pairs,
    // right for link_only and an overstatement for dedupe. The dedupe-scaled
    // figure is the one that reconciles with `expected_pairs`'s largest block.
    let surname = estimate_candidate_key(&session, &concat, &["surname".to_string()]).unwrap();
    assert_eq!(
        largest.dedupe_block_pairs(),
        surname.largest_block_pairs,
        "the largest block must agree between the two paths"
    );
    assert!(
        largest.block_count > largest.dedupe_block_pairs(),
        "n^2 ({}) must exceed C(n,2) ({})",
        largest.block_count,
        largest.dedupe_block_pairs()
    );
}

#[test]
fn top_values_are_bounded_and_break_ties_stably() {
    let (session, _settings, concat) = fixture();
    let options = ProfileOptions::default();
    let columns: Vec<String> = PROFILED.iter().map(|c| (*c).to_string()).collect();

    let top = top_values(&session, &concat, &columns, 5, &options).unwrap();
    assert_eq!(
        top.len(),
        PROFILED.len() * 5,
        "exactly k rows per column, however large the corpus"
    );

    let surname: Vec<_> = top.iter().filter(|v| v.column_name == "surname").collect();
    assert_eq!(surname[0].value.as_deref(), Some("Jones"));
    assert_eq!(surname[0].count, 23);
    assert_eq!(surname[0].rank, 1);
    assert!(
        (surname[0].share - 23.0 / 819.0).abs() < 1e-12,
        "share is over the column's NON-NULL rows"
    );

    // Davies and Taylor both appear 14 times. The order must come from the
    // value, not from however the group-by happened to emit them, or two
    // profiles of identical data would disagree.
    assert_eq!(surname[1].value.as_deref(), Some("Davies"));
    assert_eq!(surname[2].value.as_deref(), Some("Taylor"));
    assert_eq!(surname[1].count, 14);
    assert_eq!(surname[2].count, 14);
    assert_eq!(
        top_values(&session, &concat, &columns, 5, &options).unwrap(),
        top,
        "top values must be reproducible"
    );
}

#[test]
fn the_mode_reconciles_with_the_largest_block() {
    let (session, _settings, concat) = fixture();
    let key = vec!["surname".to_string()];

    let mode = modes(&session, &concat, &key, &ProfileOptions::default()).unwrap();
    assert_eq!(mode.len(), 1, "one row per column");
    let mode = &mode[0];
    assert_eq!(mode.value.as_deref(), Some("Jones"));

    // The two halves of the profile are computed by different SQL over
    // different shapes, so agreeing here is evidence rather than tautology:
    // the mode's own rows are the largest block, and C(23,2) = 253 is what a
    // dedupe run scores inside it.
    let estimate = estimate_candidate_key(&session, &concat, &key).unwrap();
    assert_eq!(estimate.largest_block_rows, mode.count);
    assert_eq!(
        estimate.largest_block_pairs,
        mode.count * (mode.count - 1) / 2,
        "the mode's block and the largest-block pair count must agree"
    );
    assert_eq!(estimate.largest_block_pairs, 253);
}

#[test]
fn uniqueness_refuses_to_answer_from_the_sketch() {
    let (session, _settings, concat) = fixture();
    // `unique_id` is the one genuinely unique column.
    let columns = vec!["unique_id".to_string(), "surname".to_string()];

    let approx = column_shape(
        &session,
        &concat,
        &columns,
        &ProfileOptions {
            exact_distinct: false,
            sample: None,
        },
    )
    .unwrap();
    let approx = read_column_shapes(&session, &approx).unwrap();
    for row in &approx {
        assert_eq!(
            row.is_unique(),
            None,
            "{}: uniqueness must be unanswerable without an exact count — the \
             ratio is clamped to 1.0, so it cannot tell a unique column from \
             one carrying a few duplicates",
            row.column_name
        );
        // The ratio is still offered, and still clamped into range.
        assert!(row.cardinality_ratio().is_some_and(|r| r <= 1.0));
    }

    let exact = column_shape(
        &session,
        &concat,
        &columns,
        &ProfileOptions {
            exact_distinct: true,
            sample: None,
        },
    )
    .unwrap();
    let exact = read_column_shapes(&session, &exact).unwrap();
    assert_eq!(by_name(&exact, "unique_id").is_unique(), Some(true));
    assert_eq!(
        by_name(&exact, "surname").is_unique(),
        Some(false),
        "371 distinct surnames over 819 rows is not unique"
    );

    // Pin what the sketch actually does here, measured rather than assumed.
    //
    // `unique_id` holds 1000 distinct values and the sketch reports 978 — it
    // UNDERSHOOTS by 2.2%, which is the direction that matters most for this
    // check: 978/1000 is 0.978, below the clamp, so `cardinality_ratio` reads
    // as "not quite unique" for a column in which every single value IS
    // unique. Overshoot (which the same sketch does on other data, and which
    // the clamp exists for) produces the opposite error, hiding real
    // duplicates behind a clamped 1.0. Both directions are wrong, which is why
    // `is_unique` declines rather than inferring from the ratio.
    //
    // Pinned because `duckdb` is an `=` dependency: a bump that moves these is
    // a deliberate, reviewable event, and this is where it surfaces.
    let uid_approx = by_name(&approx, "unique_id").distinct_approx;
    let uid_exact = by_name(&exact, "unique_id").distinct_exact.unwrap();
    assert_eq!(
        (uid_approx, uid_exact),
        (978, 1000),
        "unique_id: (approx, exact) distinct counts"
    );
    assert_ne!(
        uid_approx, uid_exact,
        "the sketch must be shown to disagree with the truth, or this test \
         is not demonstrating anything"
    );
    assert!(
        by_name(&approx, "unique_id")
            .cardinality_ratio()
            .is_some_and(|r| r < 1.0),
        "the undershoot leaves the ratio under 1.0, so it is not even clamped \
         — a ratio-based uniqueness check would simply be wrong here"
    );
    assert_eq!(by_name(&exact, "surname").distinct_exact, Some(371));
}
