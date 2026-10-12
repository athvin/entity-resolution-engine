//! Incremental-linkage golden gate: predict_between (new batch vs existing)
//! and predict_within (within the new batch) must match the committed goldens
//! on the same split of fake_1000.

mod common;

use common::{assert_table_matches_parquet, golden_dir, load_golden_text};
use dedupe_core::Settings;
use dedupe_core::incremental::{predict_between, predict_within};
use dedupe_core::predict;
use dedupe_core::session::{Session, SessionOptions, TableHandle};

fn register_parquet(session: &Session, name: &str, templated: &str) -> TableHandle {
    let path = golden_dir().join(name);
    let path = path.to_string_lossy().replace('\'', "''");
    session
        .register_table(&format!("read_parquet('{path}')"), templated)
        .unwrap()
}

const MW_COLS: &[(&str, f64)] = &[
    ("match_weight", 1e-8),
    ("match_probability", 1e-10),
    ("mw_first_name", 1e-8),
    ("mw_surname", 1e-8),
    ("mw_dob", 1e-8),
    ("mw_city", 1e-8),
    ("mw_email", 1e-8),
];
const GAMMA_COLS: &[&str] = &[
    "match_key",
    "gamma_first_name",
    "gamma_surname",
    "gamma_dob",
    "gamma_city",
    "gamma_email",
];

#[test]
fn predict_between_matches_golden() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();

    let existing_input = register_parquet(&session, "incremental_existing.parquet", "existing");
    let new_input = register_parquet(&session, "incremental_new.parquet", "new");
    let existing_concat = predict::materialize_concat(&session, &existing_input).unwrap();
    let new_concat = predict::materialize_concat(&session, &new_input).unwrap();
    // TF from the established (existing) data.
    let tf_tables = predict::materialize_tf_tables(&session, &settings, &existing_concat).unwrap();

    let between = predict_between(
        &session,
        &settings,
        &existing_concat,
        &new_concat,
        &tf_tables,
        None,
    )
    .unwrap();

    assert_table_matches_parquet(
        &session,
        &between,
        "incremental_between.parquet",
        &["unique_id_l", "unique_id_r"],
        GAMMA_COLS,
        MW_COLS,
    );
}

#[test]
fn predict_within_matches_golden() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();

    let existing_input = register_parquet(&session, "incremental_existing.parquet", "existing");
    let new_input = register_parquet(&session, "incremental_new.parquet", "new");
    let existing_concat = predict::materialize_concat(&session, &existing_input).unwrap();
    let new_concat = predict::materialize_concat(&session, &new_input).unwrap();
    let tf_tables = predict::materialize_tf_tables(&session, &settings, &existing_concat).unwrap();

    let within = predict_within(&session, &settings, &new_concat, &tf_tables, None).unwrap();

    assert_table_matches_parquet(
        &session,
        &within,
        "incremental_within.parquet",
        &["unique_id_l", "unique_id_r"],
        GAMMA_COLS,
        MW_COLS,
    );
}
