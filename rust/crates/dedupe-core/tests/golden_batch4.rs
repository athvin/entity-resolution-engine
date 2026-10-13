//! Batch-4 golden gates: completeness, column value frequencies, and
//! m-from-labels (column + pairwise table) — vs the committed goldens.

mod common;

use common::{
    FAKE_1000_SCHEMA, assert_table_matches_parquet, fake_1000_csv_path, golden_dir,
    load_golden_json, load_golden_text,
};
use dedupe_core::Settings;
use dedupe_core::model::TrainedModel;
use dedupe_core::predict;
use dedupe_core::profile;
use dedupe_core::session::{Session, SessionOptions, TableHandle};
use dedupe_core::training::m_from_labels::{
    estimate_m_from_label_column, estimate_m_from_pairwise_labels,
};

const PROF_COLS: &[&str] = &["first_name", "surname", "dob", "city", "email"];

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
fn completeness_matches_golden() {
    let (session, settings, concat) = setup();
    let cols: Vec<String> = PROF_COLS
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    let comp = profile::completeness(&session, &settings, &concat, &cols, "input_data_1").unwrap();

    let golden = load_golden_json("completeness.json");
    let golden = golden.as_array().unwrap();
    let rows = session
        .query_strings(&format!(
            "select column_name, total_null_rows, total_rows_inc_nulls, completeness from {} order by column_name",
            comp.physical
        ))
        .unwrap();
    use std::collections::HashMap;
    let gmap: HashMap<String, &serde_json::Value> = golden
        .iter()
        .map(|g| (g["column_name"].as_str().unwrap().to_string(), g))
        .collect();
    assert_eq!(rows.len(), golden.len());
    for r in rows {
        let g = gmap[&r[0]];
        assert_eq!(
            r[1].parse::<i64>().unwrap(),
            g["total_null_rows"].as_i64().unwrap(),
            "null rows {}",
            r[0]
        );
        assert_eq!(
            r[2].parse::<i64>().unwrap(),
            g["total_rows_inc_nulls"].as_i64().unwrap(),
            "total {}",
            r[0]
        );
        let c = r[3].parse::<f64>().unwrap();
        let gc = g["completeness"].as_f64().unwrap();
        assert!((c - gc).abs() <= 1e-6, "completeness {}: {c} vs {gc}", r[0]);
    }
}

#[test]
fn value_frequencies_match_golden() {
    let (session, _settings, concat) = setup();
    let cols = vec!["first_name".to_string(), "city".to_string()];
    let vf = profile::column_value_frequencies(&session, &concat, &cols).unwrap();
    assert_table_matches_parquet(
        &session,
        &vf,
        "value_frequencies.parquet",
        &["group_name", "value"],
        &[
            "value_count",
            "total_non_null_rows",
            "total_rows_inc_nulls",
            "distinct_value_count",
        ],
        &[],
    );
}

fn assert_m_matches(model: &TrainedModel, golden_name: &str) {
    let golden = load_golden_json(golden_name);
    let gcomps = golden["comparisons"].as_array().unwrap();
    for (c, gc) in model.settings.comparisons.iter().zip(gcomps) {
        let glevels = gc["comparison_levels"].as_array().unwrap();
        for (level, gl) in c.levels.iter().zip(glevels) {
            if level.is_null_level {
                continue;
            }
            if let (Some(actual), Some(expected)) = (level.m.raw(), gl["m_probability"].as_f64()) {
                assert!(
                    (actual - expected).abs() <= expected.abs() * 1e-9 + 1e-12,
                    "{}/{}: m {actual} vs {expected}",
                    c.output_column_name,
                    level.sql_condition
                );
            }
        }
    }
}

#[test]
fn m_from_label_column_matches_golden() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = session
        .register_csv(&fake_1000_csv_path(), "fake_1000", FAKE_1000_SCHEMA)
        .unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_initial.json")).unwrap();
    let mut model = TrainedModel::new(settings);
    let concat = predict::materialize_concat(&session, &input).unwrap();
    let tf = predict::materialize_tf_tables(&session, &model.settings, &concat).unwrap();
    estimate_m_from_label_column(&session, &mut model, &concat, &tf, "cluster").unwrap();
    assert_m_matches(&model, "model_m_from_label_column.json");
}

#[test]
fn m_from_pairwise_labels_matches_golden() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = session
        .register_csv(&fake_1000_csv_path(), "fake_1000", FAKE_1000_SCHEMA)
        .unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_initial.json")).unwrap();
    let mut model = TrainedModel::new(settings);
    let concat = predict::materialize_concat(&session, &input).unwrap();
    let tf = predict::materialize_tf_tables(&session, &model.settings, &concat).unwrap();

    let lp = golden_dir().join("labels_table.parquet");
    let lp = lp.to_string_lossy().replace('\'', "''");
    let labels = session
        .register_table(
            &format!(
                "(select unique_id_l, unique_id_r, clerical_match_score from read_parquet('{lp}'))"
            ),
            "labels",
        )
        .unwrap();
    estimate_m_from_pairwise_labels(&session, &mut model, &concat, &tf, &labels).unwrap();
    assert_m_matches(&model, "model_m_from_pairwise_labels.json");
}
