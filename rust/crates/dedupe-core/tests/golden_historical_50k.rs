//! Scale/diversity golden gate on a second, larger dataset (historical_50k:
//! 50,578 records, richer schema, high null rates). Full pipeline — train
//! (prior + u + EM), predict, cluster — vs the committed goldens.

mod common;

use std::path::{Path, PathBuf};

use dedupe_core::Settings;
use dedupe_core::clustering::{ClusterOptions, cluster_at_threshold};
use dedupe_core::model::TrainedModel;
use dedupe_core::predict;
use dedupe_core::session::{Session, SessionOptions, TableHandle};
use dedupe_core::training::em::{EmOptions, run_em_session};
use dedupe_core::training::estimate_u::{UOptions, estimate_u};
use dedupe_core::training::prior::estimate_prior;

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/golden/historical_50k")
}
fn dataset() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/datasets/historical_50k.parquet")
}
fn golden_text(name: &str) -> String {
    std::fs::read_to_string(golden_dir().join(name)).unwrap()
}

const DETERMINISTIC_RULES: &[&str] = &[
    r#"(l."first_name" = r."first_name") AND (l."surname" = r."surname") AND (l."dob" = r."dob")"#,
    r#"(l."postcode_fake" = r."postcode_fake") AND (l."dob" = r."dob")"#,
];
const EM_RULES: &[&str] = &[
    r#"l."first_name" = r."first_name""#,
    r#"l."surname" = r."surname""#,
];

/// These run the full pipeline on 50k records (~2 min) — gated so default
/// `cargo test` stays fast; set DEDUPE_SCALE_TESTS=1 to run.
fn scale_enabled() -> bool {
    std::env::var("DEDUPE_SCALE_TESTS").as_deref() == Ok("1")
}

fn register(session: &Session) -> TableHandle {
    let p = dataset().to_string_lossy().replace('\'', "''");
    session
        .register_table(&format!("read_parquet('{p}')"), "h50k")
        .unwrap()
}

fn diff_parquet(
    session: &Session,
    actual: &TableHandle,
    golden: &str,
    keys: &[&str],
    exact: &[&str],
    approx: &[(&str, f64)],
) {
    common::assert_table_matches_parquet_path(
        session,
        actual,
        &golden_dir().join(golden),
        keys,
        exact,
        approx,
    );
}

#[test]
fn historical_50k_training_matches_golden() {
    if !scale_enabled() {
        eprintln!("skipping historical_50k scale test (set DEDUPE_SCALE_TESTS=1)");
        return;
    }
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = register(&session);
    let settings = Settings::from_json_str(&golden_text("model_initial.json")).unwrap();
    let mut model = TrainedModel::new(settings);
    let concat = predict::materialize_concat(&session, &input).unwrap();
    let tf = predict::materialize_tf_tables(&session, &model.settings, &concat).unwrap();

    let det: Vec<String> = DETERMINISTIC_RULES
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    estimate_prior(&session, &mut model, &concat, &det, 0.7).unwrap();
    estimate_u(
        &session,
        &mut model,
        &concat,
        UOptions {
            max_pairs: 1e6,
            ..Default::default()
        },
    )
    .unwrap();
    for rule in EM_RULES {
        run_em_session(
            &session,
            &mut model,
            &concat,
            &tf,
            rule,
            EmOptions::default(),
        )
        .unwrap();
    }

    let golden: serde_json::Value = serde_json::from_str(&golden_text("model_final.json")).unwrap();
    let expected_prior = golden["probability_two_random_records_match"]
        .as_f64()
        .unwrap();
    assert!(
        (model.settings.prior - expected_prior).abs() <= expected_prior.abs() * 1e-9,
        "prior {} vs {expected_prior}",
        model.settings.prior
    );
    for (c, gc) in model
        .settings
        .comparisons
        .iter()
        .zip(golden["comparisons"].as_array().unwrap())
    {
        for (level, gl) in c
            .levels
            .iter()
            .zip(gc["comparison_levels"].as_array().unwrap())
        {
            if level.is_null_level {
                continue;
            }
            if let (Some(a), Some(e)) = (level.m.raw(), gl["m_probability"].as_f64()) {
                assert!(
                    (a - e).abs() <= e.abs() * 1e-9 + 1e-12,
                    "{} m {a} vs {e}",
                    c.output_column_name
                );
            }
            if let (Some(a), Some(e)) = (level.u.raw(), gl["u_probability"].as_f64()) {
                assert!(
                    (a - e).abs() <= e.abs() * 1e-9 + 1e-12,
                    "{} u {a} vs {e}",
                    c.output_column_name
                );
            }
        }
    }
}

#[test]
fn historical_50k_predict_and_cluster_match_golden() {
    if !scale_enabled() {
        eprintln!("skipping historical_50k scale test (set DEDUPE_SCALE_TESTS=1)");
        return;
    }
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = register(&session);
    let settings = Settings::from_json_str(&golden_text("model_final.json")).unwrap();

    let predictions = predict::run_predict(&session, &settings, &input, None).unwrap();
    diff_parquet(
        &session,
        &predictions,
        "predictions.parquet",
        &["unique_id_l", "unique_id_r", "match_key"],
        &[],
        &[("match_weight", 1e-8), ("match_probability", 1e-10)],
    );

    let concat = predict::materialize_concat(&session, &input).unwrap();
    let clusters = cluster_at_threshold(
        &session,
        &settings,
        &concat,
        &predictions,
        ClusterOptions {
            threshold_match_probability: 0.95,
        },
    )
    .unwrap();
    // The golden clusters are keyed by unique_id; our output calls it node_id.
    let renamed = session
        .register_table(
            &format!(
                "(select node_id as unique_id, cluster_id from {})",
                clusters.physical
            ),
            "clusters_renamed",
        )
        .unwrap();
    diff_parquet(
        &session,
        &renamed,
        "clusters.parquet",
        &["unique_id"],
        &["cluster_id"],
        &[],
    );
}
