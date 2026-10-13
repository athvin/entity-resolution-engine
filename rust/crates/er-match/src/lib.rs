//! The matching stage (port of `src/er/matching/*`, DesignDoc.md S4.3).
//!
//! Produces `match_scores` from `int_std_records` + a committed Splink settings
//! model, via dedupe-core's Splink-parity `predict`. Scenario models are committed
//! (`fixtures/static/model_test_v1.json`) — scenario tests never train (S4.3.2:
//! EM over base_10's 23 records would be degenerate).
//!
//! S4.3's matching contract registers FROZEN term frequency from `tf_lookup`
//! (D4/INV-SCORE: never recomputed from the corpus). dedupe-core's `run_predict`
//! computes TF from the input; a frozen-TF entry point is added there so base_10's
//! `match_scores` reproduce. The score→`match_scores` wrapper (canonical pairs,
//! `evidence`, content-hash joins, MERGE/invalidation) is ER-specific and lands
//! here on top of dedupe-core.

use dedupe_core::{Result, Settings};

/// Parse a committed Splink settings document (e.g.
/// `fixtures/static/model_test_v1.json`) into dedupe-core `Settings`.
pub fn load_model(settings_json: &str) -> Result<Settings> {
    Settings::from_json_str(settings_json)
}
