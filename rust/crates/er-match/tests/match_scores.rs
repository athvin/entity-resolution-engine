//! The matching gate: Rust scores base_10 through dedupe-core's frozen-TF predict
//! and reproduces the Python/Splink `match_scores` golden
//! (`gen_match_golden.py`) per canonical pair at 1e-10.
//!
//! Rust standardizes base_10 in-session (the same path std_hashes gates byte-exact
//! vs Python), builds the frozen TF tables from the committed `tf_lookup` snapshot,
//! predicts with the committed model, then canonicalizes + thresholds exactly as
//! `er.matching.full._selected_predictions_sql` + `score_full` do (`least`/
//! `greatest`, highest-probability dedup, `match_probability >= review_low`). The
//! probability comparison runs in SQL (DOUBLE vs DOUBLE) so no string round-trip
//! erodes the 1e-10 margin.

mod common;

use dedupe_core::predict::run_predict_with_tf;
use dedupe_core::session::{Session, SessionOptions};
use er_match::{build_frozen_tf_tables, load_model};

const MODEL_VERSION: &str = "v0001";
const TF_SNAPSHOT_ID: &str = "01JWMDTFSNAP00000000000001";
const REVIEW_LOW: f64 = 0.60; // configs/test.yaml thresholds.review_low
const TF_CSV: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/model_test_v1.tf.csv"
);
const GOLDEN_CSV: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/match_scores_base_10.csv"
);

#[test]
fn base_10_match_scores_match_python_golden() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    common::land_and_standardize_base_10(&session);

    let settings = load_model(include_str!("fixtures/model_test_v1.json")).unwrap();
    let input = session
        .register_table("int_std_records", "int_std_records")
        .unwrap();
    let tf_tables = build_frozen_tf_tables(
        &session,
        &settings,
        std::path::Path::new(TF_CSV),
        MODEL_VERSION,
        TF_SNAPSHOT_ID,
    )
    .unwrap();

    // No weight threshold at predict: take every blocked pair and apply the
    // probability threshold below, exactly as score_full's `review_low`.
    let predictions = run_predict_with_tf(&session, &settings, &input, &tf_tables, None).unwrap();

    // Canonicalize + highest-probability dedup + >= review_low, matching
    // `_selected_predictions_sql`.
    session
        .execute_batch(&format!(
            "create temp table rust_scores as
             select distinct on (rec_a_key, rec_b_key) rec_a_key, rec_b_key, match_probability
               from (
                     select least(record_key_l, record_key_r) as rec_a_key,
                            greatest(record_key_l, record_key_r) as rec_b_key,
                            match_probability, rowid as prediction_id
                       from {predictions}
                      where record_key_l <> record_key_r
                        and match_probability >= {REVIEW_LOW}
                    )
              order by rec_a_key, rec_b_key, match_probability desc, prediction_id",
            predictions = predictions.physical,
        ))
        .unwrap();

    let golden_path = GOLDEN_CSV.replace('\'', "''");
    session
        .execute_batch(&format!(
            "create temp table golden as select * from read_csv('{golden_path}', header = true, \
             columns = {{'rec_a_key': 'VARCHAR', 'rec_b_key': 'VARCHAR', 'match_probability': 'DOUBLE'}})"
        ))
        .unwrap();

    let rust_n = session.count("rust_scores").unwrap();
    let golden_n = session.count("golden").unwrap();
    assert_eq!(
        golden_n, 19,
        "golden should carry base_10's 19 scored pairs (regenerate gen_match_golden.py?)"
    );
    assert_eq!(
        rust_n, golden_n,
        "Rust scored {rust_n} pairs, golden has {golden_n}"
    );

    // Pair-set equality: no pair in one side but not the other.
    let unmatched = session
        .count(
            "(
               (select rec_a_key, rec_b_key from rust_scores
                  except
                select rec_a_key, rec_b_key from golden)
               union all
               (select rec_a_key, rec_b_key from golden
                  except
                select rec_a_key, rec_b_key from rust_scores)
             )",
        )
        .unwrap();
    assert_eq!(
        unmatched, 0,
        "{unmatched} canonical pairs differ between Rust and the golden"
    );

    // Per-pair probability parity at 1e-10 (DOUBLE vs DOUBLE, no string round-trip).
    let max_abs_diff = session
        .query_scalar_f64(
            "select coalesce(max(abs(r.match_probability - g.match_probability)), 0.0)
               from rust_scores r join golden g using (rec_a_key, rec_b_key)",
        )
        .unwrap();
    assert!(
        max_abs_diff <= 1e-10,
        "max |match_probability - golden| = {max_abs_diff:e} exceeds 1e-10"
    );
}
