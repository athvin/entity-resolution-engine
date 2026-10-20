//! Clustering golden gate: connected-components clustering must reproduce
//! the golden cluster membership exactly — first from the golden predictions
//! (isolating clustering), then end-to-end from our own predictions.

mod common;

use common::{
    FAKE_1000_SCHEMA, assert_table_matches_parquet, fake_1000_csv_path, golden_dir,
    load_golden_text,
};
use dedupe_core::Settings;
use dedupe_core::clustering::{ClusterOptions, cluster_at_threshold, clustered_with_input_data};
use dedupe_core::predict;
use dedupe_core::session::{Session, SessionOptions, TableHandle};

const THRESHOLD: f64 = 0.95;

fn setup() -> (Session, Settings, TableHandle) {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = session
        .register_csv(&fake_1000_csv_path(), "fake_1000", FAKE_1000_SCHEMA)
        .unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();
    let concat = predict::materialize_concat(&session, &input).unwrap();
    (session, settings, concat)
}

#[test]
fn clusters_from_golden_predictions_match() {
    let (session, settings, concat) = setup();

    // Register the golden predictions as the predict table.
    let predictions_path = golden_dir().join("predictions.parquet");
    let predictions_path = predictions_path.to_string_lossy().replace('\'', "''");
    let predictions = session
        .register_table(
            &format!("read_parquet('{predictions_path}')"),
            "__dedupe__predict",
        )
        .unwrap();

    let clusters = cluster_at_threshold(
        &session,
        &settings,
        &concat,
        &predictions,
        ClusterOptions {
            threshold_match_probability: THRESHOLD,
        },
    )
    .unwrap();
    let joined = clustered_with_input_data(&session, &settings, &concat, &clusters).unwrap();

    assert_table_matches_parquet(
        &session,
        &joined,
        "clusters.parquet",
        &["unique_id"],
        &["cluster_id"],
        &[],
    );
}

#[test]
fn clusters_end_to_end_match() {
    let (session, settings, concat) = setup();
    let input_concat = concat.clone();

    // Our own predictions (no threshold), then cluster at 0.95.
    let tf_tables = predict::materialize_tf_tables(&session, &settings, &concat).unwrap();
    let blocked = predict::materialize_blocked_pairs(&session, &settings, &concat).unwrap();
    let pipeline =
        predict::predict_pipeline(&settings, &concat, &tf_tables, &blocked, None).unwrap();
    let predictions = session.materialize(&pipeline).unwrap();

    let clusters = cluster_at_threshold(
        &session,
        &settings,
        &input_concat,
        &predictions,
        ClusterOptions {
            threshold_match_probability: THRESHOLD,
        },
    )
    .unwrap();
    let joined = clustered_with_input_data(&session, &settings, &input_concat, &clusters).unwrap();

    assert_table_matches_parquet(
        &session,
        &joined,
        "clusters.parquet",
        &["unique_id"],
        &["cluster_id"],
        &[],
    );
}
