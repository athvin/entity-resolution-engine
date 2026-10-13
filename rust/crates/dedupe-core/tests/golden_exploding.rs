//! Golden gate for exploding (array-membership) blocking rules. The golden
//! dataset has a LIST `tokens` column and two blocking rules — a scalar
//! `first_name` rule and an exploding `arrays_to_explode: ["tokens"]` rule —
//! exercising the full mechanism: per-rule marginal exploded id-pairs
//! tables, the `false` exclusion contributed by preceding exploding rules, and
//! the `min(match_key)` two-stage dedup that collapses cross-rule duplicate
//! pairs.

mod common;

use std::path::{Path, PathBuf};

use common::assert_table_matches_parquet_path;
use dedupe_core::Settings;
use dedupe_core::predict;
use dedupe_core::session::{Session, SessionOptions};

fn exploding_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/golden/exploding")
}

#[test]
fn exploding_predictions_match_golden() {
    let dir = exploding_dir();
    let people = dir.join("people.parquet");
    let people_str = people.to_string_lossy().replace('\'', "''");

    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    // Register the parquet (preserving the LIST `tokens` column) as the input.
    let input = session
        .register_table(&format!("read_parquet('{people_str}')"), "people")
        .unwrap();
    let settings =
        Settings::from_json_str(&std::fs::read_to_string(dir.join("model.json")).unwrap()).unwrap();

    let predictions = predict::run_predict(&session, &settings, &input, None).unwrap();

    assert_table_matches_parquet_path(
        &session,
        &predictions,
        &dir.join("predictions.parquet"),
        &["unique_id_l", "unique_id_r"],
        &["match_key", "gamma_first_name", "gamma_city"],
        &[("match_weight", 1e-8), ("match_probability", 1e-10)],
    );
}
