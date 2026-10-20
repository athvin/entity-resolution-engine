//! Estimate m from ground truth (`estimate_m_from_label_column` and
//! `estimate_m_from_pairwise_labels`).
//!
//! Known-match pairs are generated (by equality of a label column, or from a
//! labels table), their comparison vectors computed, and every pair treated as
//! a match. Then `m_i = count(level i among matches) / Σ counts over non-null
//! levels`. Only m is trained (u and the prior are untouched); values combine
//! across training runs by median. Null levels are excluded from the
//! denominator; unobserved levels become the not-observed sentinel.

use crate::blocking::BlockingRule;
use crate::error::Result;
use crate::model::{EstimateSource, ProbEstimate, TrainedModel};
use crate::predict;
use crate::session::{Session, TableHandle};
use crate::settings::Settings;
use crate::sql::pipeline::CtePipeline;
use crate::stages::block::BLOCKED_ID_PAIRS;
use crate::stages::uid_expr_for;

/// Count gammas over a comparison-vectors table and write m estimates.
fn assign_m_from_vectors(
    session: &Session,
    model: &mut TrainedModel,
    vectors: &TableHandle,
) -> Result<()> {
    let mut estimates: Vec<((usize, i32), ProbEstimate)> = Vec::new();
    for (ci, c) in model.settings.comparisons.iter().enumerate() {
        let gamma = c.gamma_column_name();
        let rows = session.query_strings(&format!(
            "select {gamma} as cvv, count(*) as cnt from {} group by {gamma}",
            vectors.physical
        ))?;
        // Counts per gamma, excluding the null level (-1) from the denominator.
        let mut counts: std::collections::BTreeMap<i32, f64> = std::collections::BTreeMap::new();
        for r in rows {
            counts.insert(r[0].parse().unwrap(), r[1].parse().unwrap());
        }
        let total: f64 = counts
            .iter()
            .filter(|(g, _)| **g != -1)
            .map(|(_, n)| n)
            .sum();
        for level in &c.levels {
            if level.is_null_level {
                continue;
            }
            let est = match counts.get(&level.gamma) {
                Some(n) if total > 0.0 => ProbEstimate::Value(n / total),
                _ => ProbEstimate::NotObserved,
            };
            estimates.push(((ci, level.gamma), est));
        }
    }
    for (key, est) in estimates {
        model.add_m_estimate(
            key,
            est,
            EstimateSource::Em {
                blocking_rule: "labels".to_string(),
            },
        );
    }
    model.populate_from_trained_values();
    Ok(())
}

/// Estimate m from a ground-truth label column: block on label equality.
pub fn estimate_m_from_label_column(
    session: &Session,
    model: &mut TrainedModel,
    concat: &TableHandle,
    tf_tables: &[TableHandle],
    label_column: &str,
) -> Result<()> {
    let uid = uid_expr_for(&model.settings);
    let rule = BlockingRule {
        index: 0,
        sql: format!("l.\"{label_column}\" = r.\"{label_column}\""),
        arrays_to_explode: Vec::new(),
    };
    let mut bp = CtePipeline::new().with_input(concat);
    bp.enqueue(
        crate::stages::block::blocked_id_pairs_sql(
            std::slice::from_ref(&rule),
            &uid,
            model.settings.link_type,
            &model.settings.source_dataset_column,
            concat.templated.as_str(),
            &[],
            None,
        ),
        BLOCKED_ID_PAIRS,
    );
    let blocked = session.materialize(&bp)?;
    let cv_pipeline =
        predict::comparison_vectors_pipeline(&model.settings, concat, tf_tables, &blocked);
    let vectors = session.materialize(&cv_pipeline)?;
    assign_m_from_vectors(session, model, &vectors)?;
    session.drop_table(&blocked)?;
    session.drop_table(&vectors)?;
    Ok(())
}

/// Estimate m from a labels table of known-match pairs (clerical score is
/// ignored — every row is treated as a match). The labels table has
/// `<uid>_l`, `<uid>_r` (and `<source_dataset>_l/_r` for link types).
pub fn estimate_m_from_pairwise_labels(
    session: &Session,
    model: &mut TrainedModel,
    concat: &TableHandle,
    tf_tables: &[TableHandle],
    labels: &TableHandle,
) -> Result<()> {
    let settings: &Settings = &model.settings;
    let uid = uid_expr_for(settings);
    let uid_col = &settings.unique_id_column;
    let id_l = uid.render_suffixed("l");
    let id_r = uid.render_suffixed("r");

    // Lower-id-on-left normalization, then a blocked-pairs table.
    let mut swap = Vec::new();
    if settings.source_dataset_required() {
        let sd = &settings.source_dataset_column;
        swap.push(format!(
            "case when {id_l} < {id_r} then \"{sd}_l\" else \"{sd}_r\" end as \"{sd}_l\""
        ));
        swap.push(format!(
            "case when {id_l} < {id_r} then \"{sd}_r\" else \"{sd}_l\" end as \"{sd}_r\""
        ));
    }
    swap.push(format!(
        "case when {id_l} < {id_r} then \"{uid_col}_l\" else \"{uid_col}_r\" end as \"{uid_col}_l\""
    ));
    swap.push(format!(
        "case when {id_l} < {id_r} then \"{uid_col}_r\" else \"{uid_col}_l\" end as \"{uid_col}_r\""
    ));
    let mut lp = CtePipeline::new().with_input(labels);
    lp.enqueue(
        format!("select {} from {}", swap.join(", "), labels.templated),
        "__dedupe__labels_prepared",
    );
    let prepared = session.materialize(&lp)?;

    let mut bp = CtePipeline::new().with_input(&prepared);
    bp.enqueue(
        format!(
            "select '0' as match_key, {id_l} as join_key_l, {id_r} as join_key_r from {}",
            prepared.templated
        ),
        BLOCKED_ID_PAIRS,
    );
    let blocked = session.materialize(&bp)?;
    let cv_pipeline = predict::comparison_vectors_pipeline(settings, concat, tf_tables, &blocked);
    let vectors = session.materialize(&cv_pipeline)?;
    assign_m_from_vectors(session, model, &vectors)?;
    session.drop_table(&prepared)?;
    session.drop_table(&blocked)?;
    session.drop_table(&vectors)?;
    Ok(())
}
