//! Hash-chunked predict must produce exactly the same result as the single-shot
//! path. We assert the chunked output against the committed goldens (which the
//! unchunked path already matches), so chunked == unchunked == golden — proving
//! `num_chunks` is a pure memory/scaling lever with no effect on the result.

mod common;

use std::path::{Path, PathBuf};

use common::{
    FAKE_1000_SCHEMA, assert_table_matches_parquet, assert_table_matches_parquet_path,
    fake_1000_csv_path, load_golden_text,
};
use dedupe_core::session::{Session, SessionOptions};
use dedupe_core::{Settings, predict};

fn exploding_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/golden/exploding")
}

#[test]
fn chunked_predict_matches_golden_fake_1000() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = session
        .register_csv(&fake_1000_csv_path(), "fake_1000", FAKE_1000_SCHEMA)
        .unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();

    // 3 chunks: the pair set is split by hash(uid_r) % 3 and unioned.
    let predictions = predict::run_predict_chunked(&session, &settings, &input, None, 3).unwrap();

    assert_table_matches_parquet(
        &session,
        &predictions,
        "predictions.parquet",
        &["unique_id_l", "unique_id_r"],
        &[
            "match_key",
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
fn chunked_predict_matches_golden_exploding() {
    // Exploding rules build per-chunk marginal tables; chunking must still land
    // every pair (and all its scalar + exploding arms) in one chunk.
    let dir = exploding_dir();
    let people = dir.join("people.parquet");
    let people_str = people.to_string_lossy().replace('\'', "''");

    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = session
        .register_table(&format!("read_parquet('{people_str}')"), "people")
        .unwrap();
    let settings =
        Settings::from_json_str(&std::fs::read_to_string(dir.join("model.json")).unwrap()).unwrap();

    let predictions = predict::run_predict_chunked(&session, &settings, &input, None, 4).unwrap();

    assert_table_matches_parquet_path(
        &session,
        &predictions,
        &dir.join("predictions.parquet"),
        &["unique_id_l", "unique_id_r"],
        &["match_key", "gamma_first_name", "gamma_city"],
        &[("match_weight", 1e-8), ("match_probability", 1e-10)],
    );
}
