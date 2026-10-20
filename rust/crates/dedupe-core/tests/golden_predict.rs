//! Predict golden gate: predict() with the pretrained golden model must
//! reproduce the golden predictions — gammas exact, match weights within 1e-8,
//! match probabilities within 1e-10.

mod common;

use common::{
    FAKE_1000_SCHEMA, assert_table_matches_parquet, fake_1000_csv_path, load_golden_text,
};
use dedupe_core::Settings;
use dedupe_core::predict;
use dedupe_core::session::{Session, SessionOptions};

#[test]
fn predictions_match_golden() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = session
        .register_csv(&fake_1000_csv_path(), "fake_1000", FAKE_1000_SCHEMA)
        .unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();

    let predictions = predict::run_predict(&session, &settings, &input, None).unwrap();

    let keys = ["unique_id_l", "unique_id_r"];
    let exact = [
        "match_key",
        "gamma_first_name",
        "gamma_surname",
        "gamma_dob",
        "gamma_city",
        "gamma_email",
    ];
    let approx: Vec<(&str, f64)> = vec![
        ("match_weight", 1e-8),
        ("match_probability", 1e-10),
        ("mw_first_name", 1e-8),
        ("mw_tf_adj_first_name", 1e-8),
        ("mw_surname", 1e-8),
        ("mw_tf_adj_surname", 1e-8),
        ("mw_dob", 1e-8),
        ("mw_city", 1e-8),
        ("mw_tf_adj_city", 1e-8),
        ("mw_email", 1e-8),
        ("mw_tf_adj_email", 1e-8),
    ];
    assert_table_matches_parquet(
        &session,
        &predictions,
        "predictions.parquet",
        &keys,
        &exact,
        &approx,
    );

    // The standalone match-weight-parts stage must match as well.
    let concat = predict::materialize_concat(&session, &input).unwrap();
    let tf_tables = predict::materialize_tf_tables(&session, &settings, &concat).unwrap();
    let blocked = predict::materialize_blocked_pairs(&session, &settings, &concat).unwrap();
    let mut p = predict::comparison_vectors_pipeline(&settings, &concat, &tf_tables, &blocked);
    p.enqueue(
        dedupe_core::stages::score::match_weight_parts_sql(&settings).unwrap(),
        dedupe_core::stages::score::DF_MATCH_WEIGHT_PARTS,
    );
    let mwp = session.materialize(&p).unwrap();
    assert_table_matches_parquet(
        &session,
        &mwp,
        "stages/__dedupe__match_weight_parts.parquet",
        &["unique_id_l", "unique_id_r", "match_key"],
        &[
            "gamma_first_name",
            "gamma_surname",
            "gamma_dob",
            "gamma_city",
            "gamma_email",
        ],
        &[
            ("mw_first_name", 1e-8),
            ("mw_tf_adj_first_name", 1e-8),
            ("mw_surname", 1e-8),
            ("mw_tf_adj_surname", 1e-8),
            ("mw_dob", 1e-8),
            ("mw_city", 1e-8),
            ("mw_tf_adj_city", 1e-8),
            ("mw_email", 1e-8),
            ("mw_tf_adj_email", 1e-8),
        ],
    );
}
