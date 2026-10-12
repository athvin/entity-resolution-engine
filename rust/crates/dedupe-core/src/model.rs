//! Trained-parameter history and combination. Multiple training estimates
//! (per comparison level) are combined by Python-`statistics.median`.

use std::collections::BTreeMap;

use crate::comparison::ProbabilitySlot;
use crate::error::Result;
use crate::settings::Settings;
use crate::settings::json::SettingsJson;

#[derive(Debug, Clone, PartialEq)]
pub enum EstimateSource {
    DeterministicPrior { recall: f64 },
    USampling,
    Em { blocking_rule: String },
}

#[derive(Debug, Clone)]
pub struct ParamEstimate {
    pub value: ProbEstimate,
    pub source: EstimateSource,
}

/// A trained estimate: either a concrete value or "level not observed".
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProbEstimate {
    Value(f64),
    NotObserved,
}

/// A flat record of one model parameter (a comparison level or the prior).
#[derive(Debug, Clone, PartialEq)]
pub struct DetailedRecord {
    pub comparison_name: String,
    pub comparison_vector_value: i32,
    pub label_for_charts: String,
    pub sql_condition: String,
    pub is_null_level: bool,
    pub m_probability: Option<f64>,
    pub u_probability: Option<f64>,
    pub bayes_factor: f64,
    pub log2_bayes_factor: f64,
}

/// Python `statistics.median`: for even n, the mean of the two middle values.
// Keep the literal `(a + b) / 2.0` form (not `f64::midpoint`) so the result
// stays bit-identical to the Python `statistics.median` runs that generated
// the golden parameter estimates — the EM/median golden gates compare to 1e-10.
#[allow(clippy::manual_midpoint)]
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

/// A key identifying one comparison level: (comparison index, gamma value).
pub type LevelKey = (usize, i32);

/// Owns a `Settings` and accumulates trained estimates, writing the combined
/// (median) values back into the compiled model.
pub struct TrainedModel {
    pub settings: Settings,
    m_history: BTreeMap<LevelKey, Vec<ParamEstimate>>,
    u_history: BTreeMap<LevelKey, Vec<ParamEstimate>>,
}

impl TrainedModel {
    pub fn new(settings: Settings) -> TrainedModel {
        TrainedModel {
            settings,
            m_history: BTreeMap::new(),
            u_history: BTreeMap::new(),
        }
    }

    pub fn set_prior(&mut self, prior: f64) {
        self.settings.prior = prior;
    }

    pub fn add_u_estimate(&mut self, key: LevelKey, value: ProbEstimate, source: EstimateSource) {
        self.u_history
            .entry(key)
            .or_default()
            .push(ParamEstimate { value, source });
    }

    pub fn add_m_estimate(&mut self, key: LevelKey, value: ProbEstimate, source: EstimateSource) {
        self.m_history
            .entry(key)
            .or_default()
            .push(ParamEstimate { value, source });
    }

    /// Combine trained values (median of concrete estimates) and write them
    /// into the compiled levels. A level whose only estimates are
    /// `NotObserved` becomes the sentinel; levels with no estimates and no
    /// fixed value are left as-is.
    pub fn populate_from_trained_values(&mut self) {
        // Combine trained estimates: median of the NUMERIC estimates only
        // (sentinel "not observed" values are filtered out). If no numeric
        // estimates remain, the slot is left unchanged so it falls back to
        // the built-in default.
        let combine = |history: &BTreeMap<LevelKey, Vec<ParamEstimate>>, key: &LevelKey| {
            history.get(key).and_then(|estimates| {
                let values: Vec<f64> = estimates
                    .iter()
                    .filter_map(|e| match e.value {
                        ProbEstimate::Value(v) => Some(v),
                        ProbEstimate::NotObserved => None,
                    })
                    .collect();
                median(&values).map(ProbabilitySlot::Value)
            })
        };

        for (ci, c) in self.settings.comparisons.iter_mut().enumerate() {
            for level in &mut c.levels {
                if level.is_null_level {
                    continue;
                }
                let key = (ci, level.gamma);
                if !level.fix_u
                    && let Some(slot) = combine(&self.u_history, &key)
                {
                    level.u = slot;
                }
                if !level.fix_m
                    && let Some(slot) = combine(&self.m_history, &key)
                {
                    level.m = slot;
                }
            }
        }
    }

    pub fn to_settings_json(&self) -> SettingsJson {
        self.settings.to_settings_json()
    }

    /// The trained model parameters as flat records — one per comparison level
    /// plus a `probability_two_random_records_match` row. This is the data
    /// behind match-weights / m-u charts in a downstream frontend.
    pub fn detailed_records(&self) -> Vec<DetailedRecord> {
        let mut out = Vec::new();
        for c in &self.settings.comparisons {
            for level in &c.levels {
                let (m, u, bf, log2_bf) = if level.is_null_level {
                    (None, None, 1.0, 0.0)
                } else {
                    let m = level.m.resolve(level.default_m);
                    let u = level.u.resolve(level.default_u);
                    (Some(m), Some(u), m / u, crate::scoring::match_weight(m, u))
                };
                out.push(DetailedRecord {
                    comparison_name: c.output_column_name.clone(),
                    comparison_vector_value: level.gamma,
                    label_for_charts: level.label_for_charts.clone(),
                    sql_condition: level.sql_condition.clone(),
                    is_null_level: level.is_null_level,
                    m_probability: m,
                    u_probability: u,
                    bayes_factor: bf,
                    log2_bayes_factor: log2_bf,
                });
            }
        }
        // The prior as a record (match_key 0).
        let lambda = self.settings.prior;
        out.push(DetailedRecord {
            comparison_name: "probability_two_random_records_match".to_string(),
            comparison_vector_value: 0,
            label_for_charts: String::new(),
            sql_condition: String::new(),
            is_null_level: false,
            m_probability: None,
            u_probability: None,
            bayes_factor: lambda / (1.0 - lambda),
            log2_bayes_factor: (lambda / (1.0 - lambda)).log2(),
        });
        out
    }

    pub fn save_to_json_file(&self, path: &std::path::Path) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.to_settings_json())?;
        std::fs::write(path, json)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_median() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[5.0]), Some(5.0));
        assert_eq!(median(&[1.0, 3.0]), Some(2.0));
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), Some(2.5));
    }
}
