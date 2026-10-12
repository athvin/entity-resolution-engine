//! Link-type golden gate: link_only and link_and_dedupe over two datasets
//! (df_a / df_b derived from fake_1000). Verifies inference (predict +
//! cluster) from the trained golden model, and full training-from-scratch
//! (prior + u + EM) reproducing the golden trained parameters.

mod common;

use common::{assert_table_matches_parquet, golden_dir, load_golden_json, load_golden_text};
use dedupe_core::Settings;
use dedupe_core::clustering::{ClusterOptions, cluster_at_threshold, clustered_with_input_data};
use dedupe_core::model::TrainedModel;
use dedupe_core::predict;
use dedupe_core::session::{Session, SessionOptions, TableHandle};
use dedupe_core::training::em::{EmOptions, run_em_session};
use dedupe_core::training::estimate_u::{UOptions, estimate_u};
use dedupe_core::training::prior::estimate_prior;

const DETERMINISTIC_RULES: &[&str] = &[
    r#"(l."first_name" = r."first_name") AND (l."dob" = r."dob")"#,
    r#"l."email" = r."email""#,
];
const EM_RULES: &[&str] = &[r#"l."first_name" = r."first_name""#, r#"l."dob" = r."dob""#];
const RECALL: f64 = 0.7;
const CLUSTER_THRESHOLD: f64 = 0.95;

fn register(session: &Session, name: &str, templated: &str) -> TableHandle {
    let path = golden_dir().join(name);
    let path = path.to_string_lossy().replace('\'', "''");
    session
        .register_table(&format!("read_parquet('{path}')"), templated)
        .unwrap()
}

fn setup(session: &Session, settings: &Settings) -> TableHandle {
    let a = register(session, "link_df_a.parquet", "df_a");
    let b = register(session, "link_df_b.parquet", "df_b");
    predict::materialize_concat_multi(session, &[(&a, "df_a"), (&b, "df_b")], settings).unwrap()
}

fn gamma_cols() -> Vec<&'static str> {
    vec![
        "match_key",
        "gamma_first_name",
        "gamma_surname",
        "gamma_dob",
        "gamma_city",
        "gamma_email",
    ]
}

fn mw_cols() -> Vec<(&'static str, f64)> {
    vec![
        ("match_weight", 1e-8),
        ("match_probability", 1e-10),
        ("mw_first_name", 1e-8),
        ("mw_surname", 1e-8),
        ("mw_dob", 1e-8),
        ("mw_city", 1e-8),
        ("mw_email", 1e-8),
    ]
}

fn run_inference(link_type: &str) {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let settings =
        Settings::from_json_str(&load_golden_text(&format!("link_{link_type}_model.json")))
            .unwrap();
    let concat = setup(&session, &settings);
    let tf = predict::materialize_tf_tables(&session, &settings, &concat).unwrap();
    let blocked = predict::materialize_blocked_pairs(&session, &settings, &concat).unwrap();
    let pipeline = predict::predict_pipeline(&settings, &concat, &tf, &blocked, None).unwrap();
    let predictions = session.materialize(&pipeline).unwrap();

    assert_table_matches_parquet(
        &session,
        &predictions,
        &format!("link_{link_type}_predictions.parquet"),
        &[
            "source_dataset_l",
            "unique_id_l",
            "source_dataset_r",
            "unique_id_r",
        ],
        &gamma_cols(),
        &mw_cols(),
    );

    // Cluster and compare membership (composite cluster_id).
    let clusters = cluster_at_threshold(
        &session,
        &settings,
        &concat,
        &predictions,
        ClusterOptions {
            threshold_match_probability: CLUSTER_THRESHOLD,
        },
    )
    .unwrap();
    let joined = clustered_with_input_data(&session, &settings, &concat, &clusters).unwrap();
    assert_table_matches_parquet(
        &session,
        &joined,
        &format!("link_{link_type}_clusters.parquet"),
        &["source_dataset", "unique_id"],
        &["cluster_id"],
        &[],
    );
}

fn run_training(link_type: &str) {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let settings = Settings::from_json_str(&load_golden_text(&format!(
        "link_{link_type}_model_initial.json"
    )))
    .unwrap();
    let mut model = TrainedModel::new(settings);
    let concat = setup(&session, &model.settings);
    let tf = predict::materialize_tf_tables(&session, &model.settings, &concat).unwrap();

    let det: Vec<String> = DETERMINISTIC_RULES
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    estimate_prior(&session, &mut model, &concat, &det, RECALL).unwrap();
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

    // Compare trained prior + m/u to the golden trained model.
    let golden = load_golden_json(&format!("link_{link_type}_model.json"));
    let expected_prior = golden["probability_two_random_records_match"]
        .as_f64()
        .unwrap();
    assert!(
        (model.settings.prior - expected_prior).abs() <= expected_prior.abs() * 1e-9,
        "{link_type}: prior {} vs {expected_prior}",
        model.settings.prior
    );
    let gcomps = golden["comparisons"].as_array().unwrap();
    for (c, gc) in model.settings.comparisons.iter().zip(gcomps) {
        let glevels = gc["comparison_levels"].as_array().unwrap();
        for (level, gl) in c.levels.iter().zip(glevels) {
            if level.is_null_level {
                continue;
            }
            for (slot, key, tol) in [
                (level.m.raw(), "m_probability", 1e-9),
                (level.u.raw(), "u_probability", 1e-9),
            ] {
                if let (Some(actual), Some(expected)) = (slot, gl[key].as_f64()) {
                    assert!(
                        (actual - expected).abs() <= expected.abs() * tol + 1e-12,
                        "{link_type} {}/{}: {key} {actual} vs {expected}",
                        c.output_column_name,
                        level.sql_condition
                    );
                }
            }
        }
    }
}

#[test]
fn link_only_inference_matches_golden() {
    run_inference("link_only");
}

#[test]
fn link_and_dedupe_inference_matches_golden() {
    run_inference("link_and_dedupe");
}

#[test]
fn link_only_training_matches_golden() {
    run_training("link_only");
}

#[test]
fn link_and_dedupe_training_matches_golden() {
    run_training("link_and_dedupe");
}
