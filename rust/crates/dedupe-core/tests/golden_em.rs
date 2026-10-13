//! EM-training golden gate: EM training must reproduce the golden
//! per-iteration m/lambda history, the deactivated-comparison set, and the
//! final median-combined model after two sessions.

mod common;

use std::collections::BTreeMap;

use common::{FAKE_1000_SCHEMA, fake_1000_csv_path, load_golden_json, load_golden_text};
use dedupe_core::Settings;
use dedupe_core::model::TrainedModel;
use dedupe_core::predict;
use dedupe_core::session::{Session, SessionOptions, TableHandle};
use dedupe_core::training::em::{EmOptions, EmSessionResult, run_em_session};

const EM_RULE_1: &str = r#"(l."first_name" = r."first_name") AND (l."surname" = r."surname")"#;
const EM_RULE_2: &str = r#"l."dob" = r."dob""#;

struct Ctx {
    session: Session,
    concat: TableHandle,
    tf_tables: Vec<TableHandle>,
}

fn setup() -> (Ctx, TrainedModel) {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = session
        .register_csv(&fake_1000_csv_path(), "fake_1000", FAKE_1000_SCHEMA)
        .unwrap();
    // Start from the model after u-estimation (prior + u already set, m untrained).
    let settings = Settings::from_json_str(&load_golden_text("model_after_u.json")).unwrap();
    let model = TrainedModel::new(settings);
    let concat = predict::materialize_concat(&session, &input).unwrap();
    let tf_tables = predict::materialize_tf_tables(&session, &model.settings, &concat).unwrap();
    (
        Ctx {
            session,
            concat,
            tf_tables,
        },
        model,
    )
}

/// golden iteration history: map (comparison, iteration, gamma) -> m_probability
fn golden_m_history(name: &str) -> BTreeMap<(String, i64, i64), f64> {
    let g = load_golden_json(name);
    let mut out = BTreeMap::new();
    for r in g["iteration_history_records"].as_array().unwrap() {
        if r["is_null_level"].as_bool().unwrap_or(false) {
            continue;
        }
        let Some(m) = r["m_probability"].as_f64() else {
            continue;
        };
        out.insert(
            (
                r["comparison_name"].as_str().unwrap().to_string(),
                r["iteration"].as_i64().unwrap(),
                r["comparison_vector_value"].as_i64().unwrap(),
            ),
            m,
        );
    }
    out
}

fn golden_lambda_history(name: &str) -> Vec<f64> {
    let g = load_golden_json(name);
    let mut rows: Vec<(i64, f64)> = g["lambda_history_records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["iteration"].as_i64().unwrap(),
                r["probability_two_random_records_match"].as_f64().unwrap(),
            )
        })
        .collect();
    rows.sort_by_key(|(i, _)| *i);
    rows.into_iter().map(|(_, v)| v).collect()
}

fn check_session(result: &EmSessionResult, golden_name: &str) {
    let g_lambda = golden_lambda_history(golden_name);
    assert_eq!(
        result.iterations.len(),
        g_lambda.len(),
        "{golden_name}: iteration count"
    );

    // lambda history, per iteration (tolerance grows slowly with iteration).
    for (i, (actual, expected)) in result.iterations.iter().zip(&g_lambda).enumerate() {
        let tol = (1e-10 * 10f64.powf(i as f64 / 10.0)).min(1e-7);
        assert!(
            (actual.lambda - expected).abs() <= expected.abs() * tol + 1e-12,
            "{golden_name}: lambda iteration {i}: {} vs {expected}",
            actual.lambda
        );
    }

    // m history per (comparison, iteration, gamma).
    let g_m = golden_m_history(golden_name);
    for (i, iter) in result.iterations.iter().enumerate() {
        for (comp, levels) in &iter.params {
            for lp in levels {
                if let Some(expected) = g_m.get(&(comp.clone(), i as i64, i64::from(lp.gamma))) {
                    let tol = (1e-10 * 10f64.powf(i as f64 / 10.0)).min(1e-7);
                    assert!(
                        (lp.m - expected).abs() <= expected.abs() * tol + 1e-12,
                        "{golden_name}: m {comp} iter {i} gamma {}: {} vs {expected}",
                        lp.gamma,
                        lp.m
                    );
                }
            }
        }
    }
}

#[test]
fn em_session_1_matches_golden() {
    let (ctx, mut model) = setup();
    let result = run_em_session(
        &ctx.session,
        &mut model,
        &ctx.concat,
        &ctx.tf_tables,
        EM_RULE_1,
        EmOptions::default(),
    )
    .unwrap();
    assert_eq!(result.deactivated, vec!["first_name", "surname"]);
    check_session(&result, "em_session_1_first_name_surname.json");
}

#[test]
fn em_session_2_matches_golden() {
    let (ctx, mut model) = setup();
    // Session 2 starts from the model AFTER session 1.
    run_em_session(
        &ctx.session,
        &mut model,
        &ctx.concat,
        &ctx.tf_tables,
        EM_RULE_1,
        EmOptions::default(),
    )
    .unwrap();
    let result = run_em_session(
        &ctx.session,
        &mut model,
        &ctx.concat,
        &ctx.tf_tables,
        EM_RULE_2,
        EmOptions::default(),
    )
    .unwrap();
    assert_eq!(result.deactivated, vec!["dob"]);
    check_session(&result, "em_session_2_dob.json");
}

#[test]
fn final_model_matches_golden() {
    let (ctx, mut model) = setup();
    for rule in [EM_RULE_1, EM_RULE_2] {
        run_em_session(
            &ctx.session,
            &mut model,
            &ctx.concat,
            &ctx.tf_tables,
            rule,
            EmOptions::default(),
        )
        .unwrap();
    }

    let golden = load_golden_json("model_final.json");
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
                    "{}/{}: final m {actual} vs {expected}",
                    c.output_column_name,
                    level.sql_condition
                );
            }
        }
    }
}
