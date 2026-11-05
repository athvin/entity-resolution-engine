//! u-estimation golden gate: prior estimation (exact), the hash-sampled
//! record set (exact), and u probabilities (default + simple paths) vs the
//! committed goldens.

use crate::common;

use common::{
    FAKE_1000_SCHEMA, assert_table_matches_parquet, fake_1000_csv_path, load_golden_json,
    load_golden_text,
};
use dedupe_core::Settings;
use dedupe_core::model::TrainedModel;
use dedupe_core::predict;
use dedupe_core::session::{Session, SessionOptions, TableHandle};
use dedupe_core::training::estimate_u::{UOptions, estimate_u};
use dedupe_core::training::prior::estimate_prior;

const DETERMINISTIC_RULES: &[&str] = &[
    r#"(l."first_name" = r."first_name") AND (l."dob" = r."dob")"#,
    "l.first_name = r.first_name and levenshtein(r.surname, l.surname) <= 2",
    r#"l."email" = r."email""#,
];
const RECALL: f64 = 0.7;

fn setup() -> (Session, TableHandle) {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = session
        .register_csv(&fake_1000_csv_path(), "fake_1000", FAKE_1000_SCHEMA)
        .unwrap();
    let concat = predict::materialize_concat(&session, &input).unwrap();
    (session, concat)
}

fn model() -> TrainedModel {
    TrainedModel::new(Settings::from_json_str(&load_golden_text("model_initial.json")).unwrap())
}

#[test]
fn prior_matches_golden() {
    let (session, concat) = setup();
    let mut m = model();
    let det: Vec<String> = DETERMINISTIC_RULES
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    let prior = estimate_prior(&session, &mut m, &concat, &det, RECALL).unwrap();

    let golden = load_golden_json("model_after_prior.json");
    let expected = golden["probability_two_random_records_match"]
        .as_f64()
        .unwrap();
    assert!(
        (prior - expected).abs() <= expected.abs() * 1e-12,
        "prior {prior} vs golden {expected}"
    );
}

/// Compare resolved u probabilities (median-combined) against a golden model,
/// within a relative tolerance.
fn assert_u_matches(m: &TrainedModel, golden_name: &str, rel: f64) {
    let golden = load_golden_json(golden_name);
    let gcomps = golden["comparisons"].as_array().unwrap();
    for (c, gc) in m.settings.comparisons.iter().zip(gcomps) {
        let glevels = gc["comparison_levels"].as_array().unwrap();
        for (level, gl) in c.levels.iter().zip(glevels) {
            if level.is_null_level {
                continue;
            }
            let expected = gl["u_probability"].as_f64();
            let actual = level.u.raw();
            match (actual, expected) {
                (Some(a), Some(e)) => assert!(
                    (a - e).abs() <= e.abs() * rel,
                    "{}/{}: u {a} vs golden {e}",
                    c.output_column_name,
                    level.sql_condition
                ),
                (None, None) => {}
                _ => panic!(
                    "{}/{}: u presence mismatch (actual {actual:?}, golden {expected:?})",
                    c.output_column_name, level.sql_condition
                ),
            }
        }
    }
}

#[test]
fn u_simple_path_matches_golden() {
    let (session, concat) = setup();
    let mut m = model();
    estimate_u(
        &session,
        &mut m,
        &concat,
        UOptions {
            max_pairs: 1e6,
            seed: None,
            min_count_per_level: None,
            num_chunks: 1,
        },
    )
    .unwrap();
    assert_u_matches(&m, "model_after_u_simple.json", 1e-12);
}

#[test]
fn u_default_path_matches_golden() {
    let (session, concat) = setup();
    let mut m = model();
    estimate_u(&session, &mut m, &concat, UOptions::default()).unwrap();
    assert_u_matches(&m, "model_after_u.json", 1e-12);
}

/// `defaulted_levels` reports exactly the parameters the scorer will take from a
/// built-in default rather than from this corpus — checked at the two stages the
/// committed goldens already capture.
///
/// Worth pinning because the fallback is otherwise invisible: training succeeds,
/// the model saves, every golden here passes, and nothing in the saved document
/// distinguishes a u measured over a million pairs from a prior about where the
/// level sits in the ladder. For an exact level on a near-unique column the two
/// can be orders of magnitude apart.
///
/// The goldens make the two halves separable, which is the real property: after
/// u-sampling every non-null level has a measured u and *none* has a measured m,
/// so a report that conflated them would fail here.
#[test]
fn defaulted_levels_tracks_what_training_has_measured() {
    // Untrained: neither parameter exists anywhere, so every non-null level is
    // reported for both. `detailed_records` supplies the denominator rather than
    // a hand-written 20, but the count is asserted so that an accessor silently
    // returning nothing cannot pass.
    let initial = model();
    let non_null = initial
        .detailed_records()
        .iter()
        // The prior travels as a record too, with no m and no u; this excludes
        // it without naming it.
        .filter(|r| !r.is_null_level && r.m_probability.is_some())
        .count();
    assert!(non_null > 0, "the fixture must have non-null levels");

    let defaulted = initial.defaulted_levels();
    assert_eq!(
        defaulted.len(),
        non_null,
        "an untrained model defaults every non-null level"
    );
    assert!(
        defaulted.iter().all(|d| d.m_defaulted && d.u_defaulted),
        "...and both of its parameters, not one"
    );
    let described: Vec<String> = defaulted
        .iter()
        .map(dedupe_core::model::DefaultedLevel::describe)
        .collect();
    assert!(
        described
            .iter()
            .any(|d| d == "email / Exact match on email (m and u)"),
        "the description must name the comparison, the level and which parameters \
         fell back: {described:?}"
    );

    // After u-sampling: u is measured everywhere, m is still a default because
    // nothing has estimated it yet.
    let after_u = TrainedModel::new(
        Settings::from_json_str(&load_golden_text("model_after_u.json")).unwrap(),
    );
    let defaulted = after_u.defaulted_levels();
    assert_eq!(
        defaulted.len(),
        non_null,
        "m is still unmeasured on every level"
    );
    assert!(
        defaulted.iter().all(|d| d.m_defaulted && !d.u_defaulted),
        "u came from the corpus and m did not — the two must not be conflated"
    );
}

#[test]
fn sampled_record_set_matches_golden() {
    // max_pairs below C(1000,2) so the deterministic hash filter fires.
    let (session, concat) = setup();

    // Re-derive the deterministic sample filter the way estimate_u does and
    // compare the selected record set to the golden one.
    let proportion = {
        let total = session.count(&concat.physical).unwrap() as f64;
        // Mirror estimate_u's arithmetic exactly (not f64::midpoint — see there).
        #[allow(clippy::manual_midpoint)]
        let sample_size = 0.5 * ((8.0_f64 * 2e5 + 1.0).sqrt() + 1.0);
        (sample_size / total).min(1.0)
    };
    assert!(proportion < 1.0, "sampling must actually fire");

    let threshold = (round_half_even(proportion * 1e9) as i64).max(1);
    let filter = format!(" AND hash(\"unique_id\") % 1000000000 < {threshold}");

    let mut p = dedupe_core::sql::pipeline::CtePipeline::new().with_input(&concat);
    p.enqueue(
        format!("select *\nfrom {}\nwhere 1=1\n{filter}", concat.templated),
        "__dedupe__concat_sample",
    );
    let sample = session.materialize(&p).unwrap();

    assert_table_matches_parquet(
        &session,
        &sample,
        "u_sample_ids.parquet",
        &["unique_id"],
        &["first_name", "surname", "dob", "city", "email", "cluster"],
        &[],
    );
}

fn round_half_even(x: f64) -> f64 {
    let floor = x.floor();
    let diff = x - floor;
    if diff < 0.5 {
        floor
    } else if diff > 0.5 {
        floor + 1.0
    } else if (floor as i64) % 2 == 0 {
        floor
    } else {
        floor + 1.0
    }
}
