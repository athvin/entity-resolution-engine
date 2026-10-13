//! Serde mirror of the settings / saved-model JSON format.
//!
//! A saved model loads here verbatim, and re-serializing produces
//! semantically identical JSON (unknown keys are preserved via `extra`
//! passthrough maps). Legacy keys (`bayes_factor_column_prefix`,
//! `salting_partitions`) are accepted and dropped on load.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkType {
    #[serde(rename = "dedupe_only")]
    DedupeOnly,
    #[serde(rename = "link_only")]
    LinkOnly,
    #[serde(rename = "link_and_dedupe")]
    LinkAndDedupe,
}

fn d_prior() -> f64 {
    0.0001
}
fn d_em_convergence() -> f64 {
    0.0001
}
fn d_max_iterations() -> u32 {
    25
}
fn d_unique_id() -> String {
    "unique_id".to_string()
}
fn d_source_dataset() -> String {
    "source_dataset".to_string()
}
fn d_true() -> bool {
    true
}
fn d_mw_prefix() -> String {
    "mw_".to_string()
}
fn d_tf_prefix() -> String {
    "tf_".to_string()
}
fn d_gamma_prefix() -> String {
    "gamma_".to_string()
}
// serde `skip_serializing_if` predicates must have signature `fn(&T) -> bool`,
// so the `&bool` is mandated by the serde API, not a style choice.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SettingsJson {
    pub link_type: LinkType,
    #[serde(default = "d_prior")]
    pub probability_two_random_records_match: f64,
    #[serde(default = "d_true")]
    pub retain_matching_columns: bool,
    #[serde(default)]
    pub retain_intermediate_calculation_columns: bool,
    #[serde(default)]
    pub additional_columns_to_retain: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql_dialect: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linker_uid: Option<String>,
    #[serde(default = "d_em_convergence")]
    pub em_convergence: f64,
    #[serde(default = "d_max_iterations")]
    pub max_iterations: u32,
    #[serde(default = "d_mw_prefix")]
    pub match_weight_column_prefix: String,
    #[serde(default = "d_tf_prefix")]
    pub term_frequency_adjustment_column_prefix: String,
    #[serde(default = "d_gamma_prefix")]
    pub comparison_vector_value_column_prefix: String,
    #[serde(default = "d_unique_id")]
    pub unique_id_column_name: String,
    #[serde(default = "d_source_dataset")]
    pub source_dataset_column_name: String,
    #[serde(default)]
    pub blocking_rules_to_generate_predictions: Vec<BlockingRuleJson>,
    #[serde(default)]
    pub comparisons: Vec<ComparisonJson>,
    /// Unknown top-level keys, preserved for round-tripping. The legacy
    /// `bayes_factor_column_prefix` lands here and is ignored.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum BlockingRuleJson {
    Sql(String),
    Detailed {
        blocking_rule: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sql_dialect: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        arrays_to_explode: Vec<String>,
        /// The legacy `salting_partitions` lands here and is ignored.
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

impl BlockingRuleJson {
    pub fn sql(&self) -> &str {
        match self {
            BlockingRuleJson::Sql(s) => s,
            BlockingRuleJson::Detailed { blocking_rule, .. } => blocking_rule,
        }
    }

    pub fn arrays_to_explode(&self) -> &[String] {
        match self {
            BlockingRuleJson::Sql(_) => &[],
            BlockingRuleJson::Detailed {
                arrays_to_explode, ..
            } => arrays_to_explode,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ComparisonJson {
    pub output_column_name: String,
    pub comparison_levels: Vec<ComparisonLevelJson>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison_description: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ComparisonLevelJson {
    pub sql_condition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_for_charts: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub m_probability: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub u_probability: Option<f64>,
    #[serde(default)]
    pub fix_m_probability: bool,
    #[serde(default)]
    pub fix_u_probability: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_null_level: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tf_adjustment_column: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tf_adjustment_weight: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tf_minimum_u_value: Option<f64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub disable_tf_exact_match_detection: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl SettingsJson {
    pub fn from_json_str(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(s)
    }

    pub fn to_json_string_pretty(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }
}
