//! Batch-3 golden gates: prediction errors (column + table), labels-table
//! accuracy, and unlinkables — vs the committed goldens.

mod common;

use common::{
    FAKE_1000_SCHEMA, assert_table_matches_parquet, fake_1000_csv_path, golden_dir,
    load_golden_text,
};
use dedupe_core::Settings;
use dedupe_core::accuracy::{
    prediction_errors_from_labels_column, prediction_errors_from_labels_table,
    truth_space_from_labels_table, unlinkables_data,
};
use dedupe_core::predict;
use dedupe_core::session::{Session, SessionOptions, TableHandle};

fn setup() -> (Session, Settings, TableHandle, Vec<TableHandle>) {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = session
        .register_csv(&fake_1000_csv_path(), "fake_1000", FAKE_1000_SCHEMA)
        .unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();
    let concat = predict::materialize_concat(&session, &input).unwrap();
    let tf = predict::materialize_tf_tables(&session, &settings, &concat).unwrap();
    (session, settings, concat, tf)
}

fn reg_labels(session: &Session) -> TableHandle {
    let p = golden_dir().join("labels_table.parquet");
    let p = p.to_string_lossy().replace('\'', "''");
    // The golden labels table uses `_l`/`_r`-suffixed id columns.
    session
        .register_table(
            &format!(
                "(select unique_id_l as unique_id_l, unique_id_r as unique_id_r, clerical_match_score from read_parquet('{p}'))"
            ),
            "labels",
        )
        .unwrap()
}

#[test]
fn prediction_errors_column_matches_golden() {
    let (session, settings, concat, tf) = setup();
    let errors = prediction_errors_from_labels_column(
        &session, &settings, &concat, &tf, "cluster", 0.5, true, true,
    )
    .unwrap();
    assert_table_matches_parquet(
        &session,
        &errors,
        "prediction_errors_column.parquet",
        &["unique_id_l", "unique_id_r"],
        &["clerical_match_score", "found_by_blocking_rules"],
        &[("match_probability", 1e-9)],
    );
}

#[test]
fn prediction_errors_table_matches_golden() {
    let (session, settings, concat, tf) = setup();
    let labels = reg_labels(&session);
    let errors = prediction_errors_from_labels_table(
        &session, &settings, &concat, &tf, &labels, 0.5, true, true,
    )
    .unwrap();
    assert_table_matches_parquet(
        &session,
        &errors,
        "prediction_errors_table.parquet",
        &["unique_id_l", "unique_id_r"],
        &[
            "clerical_match_score",
            "found_by_blocking_rules",
            "truth_status",
        ],
        &[("match_probability", 1e-9)],
    );
}

#[test]
fn accuracy_labels_table_matches_golden() {
    let (session, settings, concat, tf) = setup();
    let labels = reg_labels(&session);
    let truth =
        truth_space_from_labels_table(&session, &settings, &concat, &tf, &labels, 0.5, Some(0.1))
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
        "precision",
        "recall",
        "f1",
        "phi",
    ]
    .iter()
    .map(|c| (*c, 1e-9))
    .collect();
    assert_table_matches_parquet(
        &session,
        &truth,
        "accuracy_labels_table.parquet",
        &["truth_threshold"],
        &[],
        &metrics,
    );
}

#[test]
fn unlinkables_matches_golden() {
    let (session, settings, concat, tf) = setup();
    let ul = unlinkables_data(&session, &settings, &concat, &tf).unwrap();
    assert_table_matches_parquet(
        &session,
        &ul,
        "unlinkables.parquet",
        &["match_probability"],
        &[],
        &[("match_weight", 1e-9), ("prop", 1e-9), ("cum_prop", 1e-9)],
    );
}
