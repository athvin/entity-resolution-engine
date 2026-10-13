//! Accuracy-evaluation golden gate: the truth-space table computed from the
//! fake_1000 ground-truth `cluster` column must match the committed golden
//! baseline.

mod common;

use common::{
    FAKE_1000_SCHEMA, assert_table_matches_parquet, fake_1000_csv_path, load_golden_text,
};
use dedupe_core::Settings;
use dedupe_core::accuracy::{AccuracyOptions, truth_space_from_labels_column};
use dedupe_core::predict;
use dedupe_core::session::{Session, SessionOptions};

#[test]
fn truth_space_matches_golden() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = session
        .register_csv(&fake_1000_csv_path(), "fake_1000", FAKE_1000_SCHEMA)
        .unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();
    let concat = predict::materialize_concat(&session, &input).unwrap();
    let tf = predict::materialize_tf_tables(&session, &settings, &concat).unwrap();

    let truth_space = truth_space_from_labels_column(
        &session,
        &settings,
        &concat,
        &tf,
        &AccuracyOptions::default(),
    )
    .unwrap();

    let metrics: Vec<(&str, f64)> = [
        "match_probability",
        "total_clerical_labels",
        "p",
        "n",
        "tp",
        "tn",
        "fp",
        "fn",
        "P_rate",
        "N_rate",
        "tp_rate",
        "tn_rate",
        "fp_rate",
        "fn_rate",
        "precision",
        "recall",
        "specificity",
        "npv",
        "accuracy",
        "f1",
        "f2",
        "f0_5",
        "p4",
        "phi",
    ]
    .iter()
    .map(|c| (*c, 1e-9))
    .collect();

    assert_table_matches_parquet(
        &session,
        &truth_space,
        "accuracy_truth_space.parquet",
        &["truth_threshold"],
        &[],
        &metrics,
    );
}
