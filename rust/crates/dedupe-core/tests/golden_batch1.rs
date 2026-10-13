//! Batch-1 golden gates: deterministic_link, score_pairs, model detailed
//! records, n_largest_blocks — all against the committed goldens.

mod common;

use common::{
    FAKE_1000_SCHEMA, assert_table_matches_parquet, fake_1000_csv_path, golden_dir,
    load_golden_json, load_golden_text,
};
use dedupe_core::Settings;
use dedupe_core::blocking_analysis::n_largest_blocks;
use dedupe_core::incremental::score_pairs;
use dedupe_core::model::TrainedModel;
use dedupe_core::predict;
use dedupe_core::session::{Session, SessionOptions, TableHandle};

fn session_with_input() -> (Session, Settings, TableHandle) {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = session
        .register_csv(&fake_1000_csv_path(), "fake_1000", FAKE_1000_SCHEMA)
        .unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();
    let concat = predict::materialize_concat(&session, &input).unwrap();
    (session, settings, concat)
}

#[test]
fn deterministic_link_matches_golden() {
    let (session, settings, concat) = session_with_input();
    let tf = predict::materialize_tf_tables(&session, &settings, &concat).unwrap();
    let det = predict::deterministic_link(&session, &settings, &concat, &tf).unwrap();
    assert_table_matches_parquet(
        &session,
        &det,
        "deterministic_link.parquet",
        &["unique_id_l", "unique_id_r", "match_key"],
        &[
            "first_name_l",
            "first_name_r",
            "surname_l",
            "dob_r",
            "email_l",
        ],
        &[],
    );
}

#[test]
fn score_pairs_matches_golden() {
    let (session, settings, concat) = session_with_input();
    let tf = predict::materialize_tf_tables(&session, &settings, &concat).unwrap();
    let reg = |n: &str, t: &str| {
        let p = golden_dir().join(n);
        let p = p.to_string_lossy().replace('\'', "''");
        session
            .register_table(&format!("read_parquet('{p}')"), t)
            .unwrap()
    };
    let a_in = reg("score_pairs_a.parquet", "score_a");
    let b_in = reg("score_pairs_b.parquet", "score_b");
    let a = predict::materialize_concat(&session, &a_in).unwrap();
    let b = predict::materialize_concat(&session, &b_in).unwrap();

    let scored = score_pairs(&session, &settings, &a, &b, &tf, None).unwrap();
    assert_table_matches_parquet(
        &session,
        &scored,
        "score_pairs.parquet",
        &["unique_id_l", "unique_id_r"],
        &[
            "gamma_first_name",
            "gamma_surname",
            "gamma_dob",
            "gamma_city",
            "gamma_email",
        ],
        &[("match_weight", 1e-8), ("match_probability", 1e-10)],
    );
}

#[test]
fn detailed_records_match_golden() {
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();
    let model = TrainedModel::new(settings);
    let records = model.detailed_records();

    let golden = load_golden_json("model_detailed_records.json");
    let golden = golden.as_array().unwrap();
    // Key by (comparison_name, comparison_vector_value).
    use std::collections::HashMap;
    let gmap: HashMap<(String, i64), &serde_json::Value> = golden
        .iter()
        .map(|g| {
            (
                (
                    g["comparison_name"].as_str().unwrap().to_string(),
                    g["comparison_vector_value"].as_i64().unwrap(),
                ),
                g,
            )
        })
        .collect();
    assert_eq!(records.len(), golden.len(), "record count");
    for r in &records {
        let g = gmap
            .get(&(
                r.comparison_name.clone(),
                i64::from(r.comparison_vector_value),
            ))
            .unwrap_or_else(|| {
                panic!(
                    "missing golden row {} {}",
                    r.comparison_name, r.comparison_vector_value
                )
            });
        let close = |a: f64, b: f64, what: &str| {
            assert!(
                (a - b).abs() <= b.abs() * 1e-9 + 1e-12,
                "{} cvv {}: {what} {a} vs {b}",
                r.comparison_name,
                r.comparison_vector_value
            );
        };
        close(
            r.bayes_factor,
            g["bayes_factor"].as_f64().unwrap(),
            "bayes_factor",
        );
        close(
            r.log2_bayes_factor,
            g["log2_bayes_factor"].as_f64().unwrap(),
            "log2_bayes_factor",
        );
        if let (Some(m), Some(gm)) = (r.m_probability, g["m_probability"].as_f64()) {
            close(m, gm, "m");
        }
        if let (Some(u), Some(gu)) = (r.u_probability, g["u_probability"].as_f64()) {
            close(u, gu, "u");
        }
    }
}

#[test]
fn n_largest_blocks_matches_golden() {
    let (session, settings, concat) = session_with_input();
    let blocks = n_largest_blocks(
        &session,
        &settings,
        &concat,
        r#"l."first_name" = r."first_name""#,
        5,
    )
    .unwrap();

    let golden_path = golden_dir().join("n_largest_blocks.parquet");
    let gp = golden_path.to_string_lossy().replace('\'', "''");
    let golden = session
        .query_strings(&format!(
            "select key_0, count_l, count_r, block_count from read_parquet('{gp}') order by block_count desc"
        ))
        .unwrap();
    assert_eq!(blocks.len(), golden.len());
    for (b, g) in blocks.iter().zip(&golden) {
        assert_eq!(b.key_values[0], g[0], "key");
        assert_eq!(b.count_l.to_string(), g[1], "count_l");
        assert_eq!(b.count_r.to_string(), g[2], "count_r");
        assert_eq!(b.block_count.to_string(), g[3], "block_count");
    }
}
