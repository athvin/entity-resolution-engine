//! The §9.3 determinism experiment (batch-orchestration-design): raising
//! DuckDB threads per resource class is not a pure throughput fix — it touches
//! the contract `SessionOptions::deterministic()` exists for. This fixture is
//! the gate: predict over the golden corpus must render byte-identically at
//! one and six threads, or no class may set threads above one
//! (`NodePolicy::threads` is the per-task opt-out for a stage that is not
//! order-stable).

use crate::common;

use common::{FAKE_1000_SCHEMA, fake_1000_csv_path, load_golden_text};
use dedupe_core::Settings;
use dedupe_core::predict;
use dedupe_core::session::{Session, SessionOptions};

/// The float-bearing prediction columns, rendered as text (a VARCHAR cast is a
/// byte-equality oracle, the same shape `lake_e2e`'s `active_scores` uses).
fn predictions_rendered(threads: u32) -> Vec<Vec<String>> {
    let session =
        Session::open_in_memory(SessionOptions::deterministic().with_threads(threads)).unwrap();
    let input = session
        .register_csv(&fake_1000_csv_path(), "fake_1000", FAKE_1000_SCHEMA)
        .unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();
    let predictions = predict::run_predict(&session, &settings, &input, None).unwrap();
    session
        .query_strings(&format!(
            "SELECT unique_id_l, unique_id_r, match_key,
                    cast(match_weight AS varchar), cast(match_probability AS varchar)
             FROM {}
             ORDER BY unique_id_l, unique_id_r, match_key",
            predictions.physical
        ))
        .unwrap()
}

#[test]
fn predictions_are_byte_identical_at_one_and_six_threads() {
    let one = predictions_rendered(1);
    let six = predictions_rendered(6);
    assert!(
        !one.is_empty(),
        "the corpus scored nothing — the probe is vacuous"
    );
    assert_eq!(
        one, six,
        "six-thread predict diverged from one thread: an aggregation in the \
         scoring path is not order-stable, and the affected stage must keep \
         threads = 1 by policy (batch-orchestration-design §9.3)"
    );
}
