//! Compiled comparison model: the validated, gamma-assigned form of the
//! `comparisons` section of the settings JSON.

use crate::error::{DedupeError, Result};
use crate::settings::json::{ComparisonJson, ComparisonLevelJson};
use crate::sql::columns;

/// Input guard:
/// m/u probabilities below this are rejected at construction, and clamped to
/// this floor before `log2` during scoring.
pub const M_U_CLAMP_MIN: f64 = 1e-300;

/// The "level not observed" sentinel reads as this value.
pub const LEVEL_NOT_OBSERVED_VALUE: f64 = 1e-6;

#[derive(Debug, Clone, PartialEq)]
pub enum ProbabilitySlot {
    /// No value available (untrained, not supplied in settings). Reads as the
    /// built-in default for the level's position (see `default_m_values` /
    /// `default_u_values`).
    Unset,
    /// A concrete probability (from settings JSON or training).
    Value(f64),
    /// Level never observed during training;
    /// reads as 1e-6 regardless of defaults.
    NotObserved,
}

impl ProbabilitySlot {
    /// Resolve the slot given the level's default:
    /// `Value` wins; `NotObserved` -> 1e-6; `Unset` -> the supplied default.
    pub fn resolve(&self, default: f64) -> f64 {
        match self {
            ProbabilitySlot::Unset => default,
            ProbabilitySlot::Value(v) => *v,
            ProbabilitySlot::NotObserved => LEVEL_NOT_OBSERVED_VALUE,
        }
    }

    pub fn raw(&self) -> Option<f64> {
        match self {
            ProbabilitySlot::Value(v) => Some(*v),
            _ => None,
        }
    }
}

/// Default m values: 0.95 on the top (highest-gamma) level, the
/// remaining 0.05 split evenly, indexed by gamma 0..n-1.
pub fn default_m_values(num_levels: usize) -> Vec<f64> {
    let n = num_levels as f64;
    let split = (1.0 - 0.95) / (n - 1.0);
    let mut v = vec![split; num_levels - 1];
    v.push(0.95);
    v
}

fn interpolate(start: f64, end: f64, num_elements: usize) -> Vec<f64> {
    let steps = (num_elements - 1) as f64;
    let step = (end - start) / steps;
    (0..num_elements)
        .map(|i| start + (i as f64) * step)
        .collect()
}

/// Default u values: `m / 2^w` where match weights interpolate
/// from -5 to 3 across the non-top levels, with 10 for the top level.
pub fn default_u_values(num_levels: usize) -> Vec<f64> {
    let m_vals = default_m_values(num_levels);
    let mut match_weights = if num_levels == 2 {
        vec![-5.0]
    } else {
        interpolate(-5.0, 3.0, num_levels - 1)
    };
    match_weights.push(10.0);
    m_vals
        .iter()
        .zip(match_weights.iter())
        .map(|(m, w)| m / 2f64.powf(*w))
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct TfSpec {
    pub column: String,
    /// `tf_adjustment_weight`, default 1.0.
    pub weight: f64,
    /// `tf_minimum_u_value`, default 0.0.
    pub minimum_u_value: f64,
    /// `disable_tf_exact_match_detection`, default false.
    pub disable_exact_match_detection: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComparisonLevel {
    pub sql_condition: String,
    pub label_for_charts: String,
    /// -1 for null levels; otherwise counts DOWN from `n_nonnull_levels - 1`
    /// in declaration order (ELSE, declared last, gets 0).
    pub gamma: i32,
    pub is_null_level: bool,
    pub is_else: bool,
    pub m: ProbabilitySlot,
    pub u: ProbabilitySlot,
    /// Built-in defaults for this level's gamma position; used when m/u is Unset.
    pub default_m: f64,
    pub default_u: f64,
    pub fix_m: bool,
    pub fix_u: bool,
    pub tf: Option<TfSpec>,
    /// Base column names referenced by `sql_condition` (order of appearance,
    /// `_l`/`_r` stripped, deduped).
    pub input_columns: Vec<String>,
    /// `Some(cols)` iff the condition is a conjunction of bare-column
    /// equalities (the exact-match shape). ELSE levels are never exact.
    pub exact_match_columns: Option<Vec<String>>,
}

impl ComparisonLevel {
    pub fn has_tf_adjustments(&self) -> bool {
        self.tf.is_some()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    pub output_column_name: String,
    pub comparison_description: Option<String>,
    pub levels: Vec<ComparisonLevel>,
    /// Union of level input columns, order-preserved.
    pub input_columns: Vec<String>,
    gamma_prefix: String,
    mw_prefix: String,
    tf_prefix: String,
}

impl Comparison {
    pub fn compile(
        j: &ComparisonJson,
        gamma_prefix: &str,
        mw_prefix: &str,
        tf_prefix: &str,
    ) -> Result<Comparison> {
        let name = &j.output_column_name;
        if j.comparison_levels.is_empty() {
            return Err(DedupeError::settings(format!(
                "comparison '{name}' has no comparison_levels"
            )));
        }

        let mut levels: Vec<ComparisonLevel> = j
            .comparison_levels
            .iter()
            .map(|lj| compile_level(lj, name))
            .collect::<Result<_>>()?;

        // Gamma assignment: null levels get -1
        // and do not consume a number; others count DOWN from n_nonnull - 1.
        let n_nonnull = levels.iter().filter(|l| !l.is_null_level).count();
        if n_nonnull == 0 {
            return Err(DedupeError::settings(format!(
                "comparison '{name}' has only null levels"
            )));
        }
        let mut counter = n_nonnull as i32 - 1;
        let default_m = default_m_values(n_nonnull);
        let default_u = default_u_values(n_nonnull);
        for level in &mut levels {
            if level.is_null_level {
                level.gamma = -1;
            } else {
                level.gamma = counter;
                counter -= 1;
                level.default_m = default_m[level.gamma as usize];
                level.default_u = default_u[level.gamma as usize];
            }
        }

        let n_else = levels.iter().filter(|l| l.is_else).count();
        if n_else != 1 {
            return Err(DedupeError::settings(format!(
                "comparison '{name}' must have exactly one ELSE level (found {n_else})"
            )));
        }
        if !levels.last().unwrap().is_else {
            return Err(DedupeError::settings(format!(
                "comparison '{name}': the ELSE level must be declared last"
            )));
        }

        let mut input_columns: Vec<String> = Vec::new();
        for level in &levels {
            for c in &level.input_columns {
                if !input_columns.contains(c) {
                    input_columns.push(c.clone());
                }
            }
        }

        Ok(Comparison {
            output_column_name: name.clone(),
            comparison_description: j.comparison_description.clone(),
            levels,
            input_columns,
            gamma_prefix: gamma_prefix.to_string(),
            mw_prefix: mw_prefix.to_string(),
            tf_prefix: tf_prefix.to_string(),
        })
    }

    fn sanitised_name(&self) -> String {
        self.output_column_name.replace(' ', "_")
    }

    pub fn gamma_column_name(&self) -> String {
        format!("{}{}", self.gamma_prefix, self.sanitised_name())
    }

    pub fn mw_column_name(&self) -> String {
        format!("{}{}", self.mw_prefix, self.sanitised_name())
    }

    pub fn mw_tf_adj_column_name(&self) -> String {
        format!(
            "{}{}adj_{}",
            self.mw_prefix,
            self.tf_prefix,
            self.sanitised_name()
        )
    }

    pub fn has_tf_adjustments(&self) -> bool {
        self.levels.iter().any(ComparisonLevel::has_tf_adjustments)
    }

    /// TF-adjusted input columns of this comparison, deduped preserving order.
    pub fn tf_input_columns(&self) -> Vec<String> {
        let mut out = Vec::new();
        for level in &self.levels {
            if let Some(tf) = &level.tf
                && !out.contains(&tf.column)
            {
                out.push(tf.column.clone());
            }
        }
        out
    }
}

fn validate_probability(p: Option<f64>, what: &str, ctx: &str) -> Result<ProbabilitySlot> {
    match p {
        None => Ok(ProbabilitySlot::Unset),
        Some(v) => {
            if !(M_U_CLAMP_MIN..=1.0).contains(&v) {
                return Err(DedupeError::settings(format!(
                    "{ctx}: {what} must be in [{M_U_CLAMP_MIN:e}, 1.0], got {v}"
                )));
            }
            Ok(ProbabilitySlot::Value(v))
        }
    }
}

fn compile_level(lj: &ComparisonLevelJson, comparison_name: &str) -> Result<ComparisonLevel> {
    let ctx = format!(
        "comparison '{comparison_name}' level '{}'",
        lj.sql_condition
    );
    let is_else = columns::is_else_condition(&lj.sql_condition);
    if is_else && lj.is_null_level {
        return Err(DedupeError::settings(format!(
            "{ctx}: ELSE level cannot be a null level"
        )));
    }

    let input_columns = columns::input_columns(&lj.sql_condition, &ctx)?;
    let exact_match_columns = if is_else || lj.is_null_level {
        None
    } else {
        columns::exact_match_columns(&lj.sql_condition, &ctx)?
    };

    let tf = lj.tf_adjustment_column.as_ref().map(|col| TfSpec {
        column: col.clone(),
        weight: lj.tf_adjustment_weight.unwrap_or(1.0),
        minimum_u_value: lj.tf_minimum_u_value.unwrap_or(0.0),
        disable_exact_match_detection: lj.disable_tf_exact_match_detection,
    });

    Ok(ComparisonLevel {
        sql_condition: lj.sql_condition.clone(),
        label_for_charts: lj
            .label_for_charts
            .clone()
            .unwrap_or_else(|| lj.sql_condition.clone()),
        gamma: 0, // assigned by Comparison::compile
        is_null_level: lj.is_null_level,
        is_else,
        m: validate_probability(lj.m_probability, "m_probability", &ctx)?,
        u: validate_probability(lj.u_probability, "u_probability", &ctx)?,
        default_m: 0.0, // set after gamma assignment in Comparison::compile
        default_u: 0.0,
        fix_m: lj.fix_m_probability,
        fix_u: lj.fix_u_probability,
        tf,
        input_columns,
        exact_match_columns,
    })
}
