//! The matching stage (port of `src/er/matching/*`, DesignDoc.md S4.3).
//!
//! Produces `match_scores` from `int_std_records` + a committed Splink settings
//! model, via dedupe-core's Splink-parity `predict`. Scenario models are committed
//! (`fixtures/static/model_test_v1.json`) — scenario tests never train (S4.3.2:
//! EM over base_10's 23 records would be degenerate).
//!
//! S4.3's matching contract registers FROZEN term frequency from `tf_lookup`
//! (D4/INV-SCORE: never recomputed from the corpus). dedupe-core's `run_predict`
//! computes TF from the input; the FROZEN-TF path here builds the TF tables from a
//! `tf_lookup` snapshot and scores through dedupe-core's additive
//! `run_predict_with_tf`, so base_10's `match_scores` reproduce regardless of which
//! records are present in a batch. The score->`match_scores` wrapper (canonical
//! pairs, `evidence`, content-hash joins, MERGE/invalidation) is ER-specific and
//! lands here on top of dedupe-core.

use std::path::Path;

use dedupe_core::session::{Session, TableHandle};
use dedupe_core::sql::pipeline::CtePipeline;
use dedupe_core::stages::tf::tf_table_name;
use dedupe_core::{Result, Settings};

/// Parse a committed Splink settings document (e.g.
/// `fixtures/static/model_test_v1.json`) into dedupe-core `Settings`.
pub fn load_model(settings_json: &str) -> Result<Settings> {
    Settings::from_json_str(settings_json)
}

/// SQL-literal escape: double any single quote.
fn lit(s: &str) -> String {
    s.replace('\'', "''")
}

/// Build dedupe-core's per-column frozen TF tables from a `tf_lookup` snapshot
/// CSV (header `model_version,tf_snapshot_id,column_name,value,tf_value`), ready
/// to hand to [`dedupe_core::predict::run_predict_with_tf`].
///
/// One table is produced per [`Settings::tf_columns`], restricted to the given
/// `model_version`/`tf_snapshot_id`, named [`tf_table_name`] and shaped exactly
/// like dedupe-core's computed TF tables: a `"{column}"` value column (joined
/// onto the concat) and a `"{tf_prefix}{column}"` frozen-value column. The raw
/// `value` stays VARCHAR so it joins the standardized (string) corpus; `tf_value`
/// is cast to `float8` via DuckDB from its decimal text — the same round-to-
/// nearest the Python side gets reading the same snapshot, so the frozen weights
/// are bit-reproducible.
///
/// # Errors
/// Propagates any dedupe-core session/SQL error (CSV read, materialization).
pub fn build_frozen_tf_tables(
    session: &Session,
    settings: &Settings,
    tf_csv_path: &Path,
    model_version: &str,
    tf_snapshot_id: &str,
) -> Result<Vec<TableHandle>> {
    let src = session.register_csv(
        tf_csv_path,
        "frozen_tf_lookup",
        &[
            ("model_version", "VARCHAR"),
            ("tf_snapshot_id", "VARCHAR"),
            ("column_name", "VARCHAR"),
            ("value", "VARCHAR"),
            ("tf_value", "VARCHAR"),
        ],
    )?;
    let (mv, snap) = (lit(model_version), lit(tf_snapshot_id));

    settings
        .tf_columns()
        .iter()
        .map(|col| {
            let (name, prefix) = (lit(col), lit(&settings.tf_prefix));
            let mut p = CtePipeline::new().with_input(&src);
            p.enqueue(
                format!(
                    "select\n    value as \"{col}\",\n    cast(tf_value as float8) as \
                     \"{prefix}{col}\"\nfrom {src}\nwhere model_version = '{mv}'\n  and \
                     tf_snapshot_id = '{snap}'\n  and column_name = '{name}'",
                    src = src.templated,
                ),
                tf_table_name(col),
            );
            session.materialize(&p)
        })
        .collect()
}
