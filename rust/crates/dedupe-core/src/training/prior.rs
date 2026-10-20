//! `estimate_probability_two_random_records_match` via deterministic rules.
//!
//! Counts deduped pairs produced by the deterministic rules (same
//! "AND NOT preceding" dedup as blocking), divides by total possible
//! comparisons, and scales up by recall:
//!   prob = (num_observed / recall) / num_total_comparisons

use crate::blocking::BlockingRule;
use crate::error::{DedupeError, Result};
use crate::model::TrainedModel;
use crate::session::{Session, TableHandle};
use crate::settings::json::LinkType;
use crate::sql::pipeline::CtePipeline;
use crate::stages::block::blocked_id_pairs_sql;
use crate::stages::uid_expr_for;

/// Total possible comparisons (the cartesian count) by link type:
/// - dedupe_only / link_and_dedupe: `N*(N-1)/2` over the concat
/// - link_only: `((Σnᵢ)² − Σnᵢ²)/2` over per-dataset counts
pub fn total_possible_comparisons(
    session: &Session,
    concat: &TableHandle,
    settings: &crate::settings::Settings,
) -> Result<f64> {
    match settings.link_type {
        LinkType::DedupeOnly | LinkType::LinkAndDedupe => {
            let n = session.count(&concat.physical)? as f64;
            Ok(n * (n - 1.0) / 2.0)
        }
        LinkType::LinkOnly => {
            let rows = session.query_strings(&format!(
                "select count(*) from {} group by \"{}\"",
                concat.physical, settings.source_dataset_column
            ))?;
            let counts: Vec<f64> = rows.iter().map(|r| r[0].parse::<f64>().unwrap()).collect();
            let sum: f64 = counts.iter().sum();
            let sum_sq: f64 = counts.iter().map(|c| c * c).sum();
            Ok((sum * sum - sum_sq) / 2.0)
        }
    }
}

/// Estimate the prior from deterministic rules and write it into the model.
pub fn estimate_prior(
    session: &Session,
    model: &mut TrainedModel,
    concat: &TableHandle,
    deterministic_rules: &[String],
    recall: f64,
) -> Result<f64> {
    if recall > 1.0 || recall <= 0.0 {
        return Err(DedupeError::settings(format!(
            "recall must be in (0, 1], got {recall}"
        )));
    }

    let num_total_comparisons = total_possible_comparisons(session, concat, &model.settings)?;

    let rules: Vec<BlockingRule> = deterministic_rules
        .iter()
        .enumerate()
        .map(|(i, sql)| BlockingRule {
            index: i,
            sql: sql.clone(),
            arrays_to_explode: Vec::new(),
        })
        .collect();

    let uid = uid_expr_for(&model.settings);
    let mut p = CtePipeline::new().with_input(concat);
    p.enqueue(
        blocked_id_pairs_sql(
            &rules,
            &uid,
            model.settings.link_type,
            &model.settings.source_dataset_column,
            concat.templated.as_str(),
            &[],
            None,
        ),
        "__dedupe__deterministic_blocked",
    );
    let blocked = session.materialize(&p)?;
    let num_observed = session.count(&blocked.physical)? as f64;
    session.drop_table(&blocked)?;

    if num_observed > num_total_comparisons * recall {
        return Err(DedupeError::Other(format!(
            "deterministic rules produced more matches ({num_observed}) than recall {recall} \
             allows; recall must be at least {:.2}",
            num_observed / num_total_comparisons
        )));
    }

    let prob = (num_observed / recall) / num_total_comparisons;
    model.set_prior(prob);
    Ok(prob)
}
