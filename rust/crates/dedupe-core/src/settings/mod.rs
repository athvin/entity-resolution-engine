//! Settings: the serde JSON mirror (`json`) and the compiled, validated form
//! used by the engine.

pub mod json;

use crate::blocking::BlockingRule;
use crate::comparison::{Comparison, ProbabilitySlot};
use crate::error::{DedupeError, Result};
use crate::settings::json::{BlockingRuleJson, LinkType, SettingsJson};

#[derive(Debug, Clone)]
pub struct Settings {
    /// The original JSON, retained for round-trip re-emission (unknown keys
    /// survive here).
    pub json: SettingsJson,
    pub link_type: LinkType,
    pub unique_id_column: String,
    pub source_dataset_column: String,
    pub retain_matching_columns: bool,
    pub retain_intermediate: bool,
    pub additional_columns_to_retain: Vec<String>,
    pub gamma_prefix: String,
    pub mw_prefix: String,
    pub tf_prefix: String,
    pub prior: f64,
    pub em_convergence: f64,
    pub max_iterations: u32,
    pub comparisons: Vec<Comparison>,
    pub blocking_rules: Vec<BlockingRule>,
}

impl Settings {
    pub fn from_json_str(s: &str) -> Result<Settings> {
        let json: SettingsJson = serde_json::from_str(s)?;
        Settings::compile(json)
    }

    pub fn from_json_file(path: &std::path::Path) -> Result<Settings> {
        Settings::from_json_str(&std::fs::read_to_string(path)?)
    }

    pub fn compile(json: SettingsJson) -> Result<Settings> {
        let prior = json.probability_two_random_records_match;
        // A prior of 1.0 would emit an invalid `inf` literal into SQL; we
        // reject it (and 0, which is log2(0)).
        if !(0.0..1.0).contains(&prior) || prior == 0.0 {
            return Err(DedupeError::settings(format!(
                "probability_two_random_records_match must be in (0, 1), got {prior}"
            )));
        }

        let comparisons: Vec<Comparison> = json
            .comparisons
            .iter()
            .map(|cj| {
                Comparison::compile(
                    cj,
                    &json.comparison_vector_value_column_prefix,
                    &json.match_weight_column_prefix,
                    &json.term_frequency_adjustment_column_prefix,
                )
            })
            .collect::<Result<_>>()?;

        let blocking_rules: Vec<BlockingRule> = json
            .blocking_rules_to_generate_predictions
            .iter()
            .enumerate()
            .map(|(i, br)| compile_blocking_rule(i, br))
            .collect();

        Ok(Settings {
            link_type: json.link_type,
            unique_id_column: json.unique_id_column_name.clone(),
            source_dataset_column: json.source_dataset_column_name.clone(),
            retain_matching_columns: json.retain_matching_columns,
            retain_intermediate: json.retain_intermediate_calculation_columns,
            additional_columns_to_retain: json.additional_columns_to_retain.clone(),
            gamma_prefix: json.comparison_vector_value_column_prefix.clone(),
            mw_prefix: json.match_weight_column_prefix.clone(),
            tf_prefix: json.term_frequency_adjustment_column_prefix.clone(),
            prior,
            em_convergence: json.em_convergence,
            max_iterations: json.max_iterations,
            comparisons,
            blocking_rules,
            json,
        })
    }

    /// Re-emit settings JSON reflecting the CURRENT compiled state (prior and
    /// per-level m/u written back), in the saved-model JSON format.
    pub fn to_settings_json(&self) -> SettingsJson {
        let mut out = self.json.clone();
        out.probability_two_random_records_match = self.prior;
        for (cj, c) in out.comparisons.iter_mut().zip(&self.comparisons) {
            for (lj, l) in cj.comparison_levels.iter_mut().zip(&c.levels) {
                if !l.is_null_level {
                    if let ProbabilitySlot::Value(v) = l.m {
                        lj.m_probability = Some(v);
                    }
                    if let ProbabilitySlot::Value(v) = l.u {
                        lj.u_probability = Some(v);
                    }
                }
            }
        }
        out
    }

    /// All term-frequency columns across comparisons, deduped preserving order.
    pub fn tf_columns(&self) -> Vec<String> {
        let mut out = Vec::new();
        for c in &self.comparisons {
            for col in c.tf_input_columns() {
                if !out.contains(&col) {
                    out.push(col);
                }
            }
        }
        out
    }

    /// Whether the composite `source_dataset -__- unique_id` key is required.
    pub fn source_dataset_required(&self) -> bool {
        self.link_type != LinkType::DedupeOnly
    }
}

fn compile_blocking_rule(index: usize, br: &BlockingRuleJson) -> BlockingRule {
    BlockingRule {
        index,
        sql: br.sql().to_string(),
        arrays_to_explode: br.arrays_to_explode().to_vec(),
    }
}
