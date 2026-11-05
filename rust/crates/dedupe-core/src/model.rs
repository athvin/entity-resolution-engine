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
    /// into the compiled levels.
    ///
    /// A level whose only estimates are `NotObserved` is left **as-is**, so it
    /// falls back to its built-in default — the not-observed estimates are
    /// filtered out and `median` of an empty slice is `None`. This comment used
    /// to say such a level "becomes the sentinel", which the code below has
    /// never done; the claim outlived the variant it described, and it is the
    /// reason a later design doc concluded that an unobserved level scores at
    /// u = 1e-6.
    ///
    /// Falling back to the default is the better behaviour and worth keeping on
    /// purpose: the default is a considered prior for the level's POSITION in
    /// the ladder, while 1e-6 asserts "these values practically never agree by
    /// chance" on the strength of having seen no evidence either way.
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

    /// The non-null levels whose `m` or `u` is still [`ProbabilitySlot::Unset`]
    /// after training, so the scorer reads the level's built-in default instead
    /// of a value measured on this corpus.
    ///
    /// **Why this is worth reporting at all.** Falling back to the default is
    /// the right behaviour — see [`TrainedModel::populate_from_trained_values`],
    /// which explains why it beats a sentinel — but it is *silent*. Training
    /// succeeds, the model saves, every golden passes, and nothing distinguishes
    /// a parameter measured on a million pairs from one that is a prior about
    /// where the level sits in the ladder. For most levels the prior is fine.
    /// For an exact-match level on a near-unique column it can be orders of
    /// magnitude out, because the true u of an equality is that corpus's own
    /// collision probability.
    ///
    /// This is the post-hoc half of a pair: `er-server`'s publish advisories
    /// *predict* which `exact` levels the u-sample will not measure, from the
    /// profile alone and before any training. This reports which ones actually
    /// came back unmeasured, which is what makes that prediction checkable
    /// rather than merely plausible.
    ///
    /// Read off the final slots rather than the estimate history on purpose: the
    /// question is what the scorer will use, and a level pinned by `fix_m` /
    /// `fix_u` or supplied in the settings JSON is a declared value, not a
    /// fallback. `Unset` is the one state that means "the default is in force".
    #[must_use]
    pub fn defaulted_levels(&self) -> Vec<DefaultedLevel> {
        let mut out = Vec::new();
        for c in &self.settings.comparisons {
            for level in &c.levels {
                // The null level has no m or u by construction — `detailed_records`
                // reports `None` for both — so it is not a fallback.
                if level.is_null_level {
                    continue;
                }
                let m_defaulted = level.m.raw().is_none();
                let u_defaulted = level.u.raw().is_none();
                if m_defaulted || u_defaulted {
                    out.push(DefaultedLevel {
                        comparison_name: c.output_column_name.clone(),
                        comparison_vector_value: level.gamma,
                        label_for_charts: level.label_for_charts.clone(),
                        m_defaulted,
                        u_defaulted,
                    });
                }
            }
        }
        out
    }

    pub fn save_to_json_file(&self, path: &std::path::Path) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.to_settings_json())?;
        std::fs::write(path, json)?;
        Ok(())
    }
}

/// One comparison level whose m or u came from the built-in default rather than
/// from training. See [`TrainedModel::defaulted_levels`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultedLevel {
    pub comparison_name: String,
    pub comparison_vector_value: i32,
    pub label_for_charts: String,
    /// `m` is the default: training found no matching pairs at this level.
    pub m_defaulted: bool,
    /// `u` is the default: the u-sample never saw this level. On an exact level
    /// this usually means the column is too near-unique for random pairs to
    /// agree at the configured `u_max_pairs`.
    pub u_defaulted: bool,
}

impl DefaultedLevel {
    /// `email / Exact match on email (u)` — the comparison, the level, and which
    /// parameters fell back, in one line fit for a CLI or a task message.
    #[must_use]
    pub fn describe(&self) -> String {
        let which = match (self.m_defaulted, self.u_defaulted) {
            (true, true) => "m and u",
            (true, false) => "m",
            (false, true) => "u",
            // Not constructed: `defaulted_levels` only emits a level when at
            // least one is defaulted. Spelled out rather than `unreachable!`,
            // because a panic in a reporting path would be worse than a dull
            // string.
            (false, false) => "nothing",
        };
        format!(
            "{} / {} ({which})",
            self.comparison_name, self.label_for_charts
        )
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

    /// A level EM never observed keeps its DEFAULT, and is not driven to the
    /// near-impossible 1e-6.
    ///
    /// Asserted because nothing asserted it, and the gap is what let a false
    /// claim survive in two places at once: this function's own doc comment
    /// said such a level "becomes the sentinel", and this feature's design doc
    /// reasoned from that to "an unobserved level produces a large spurious
    /// match weight, and nothing warns". Neither was true of trained output —
    /// `combine` filters the not-observed estimates out, `median` of nothing is
    /// `None`, and the slot is left alone.
    ///
    /// The distinction is worth a test rather than a comment because the two
    /// behaviours are far apart in consequence. The default is a considered
    /// prior for the level's position in the ladder; 1e-6 asserts that these
    /// values practically never agree by chance, on the strength of having seen
    /// no evidence either way, and `log2(m/u)` turns that into a large positive
    /// weight for a level nobody measured.
    #[test]
    fn a_never_observed_level_falls_back_to_its_default() {
        let mut history: BTreeMap<LevelKey, Vec<ParamEstimate>> = BTreeMap::new();
        let key: LevelKey = (0, 2);
        history.insert(
            key,
            vec![
                ParamEstimate {
                    value: ProbEstimate::NotObserved,
                    source: EstimateSource::Em {
                        blocking_rule: "l.email = r.email".to_string(),
                    },
                },
                ParamEstimate {
                    value: ProbEstimate::NotObserved,
                    source: EstimateSource::Em {
                        blocking_rule: "l.phone = r.phone".to_string(),
                    },
                },
            ],
        );

        // The same arithmetic `populate_from_trained_values` applies: the
        // not-observed estimates are filtered, leaving nothing to take a
        // median of, so there is no slot to write and the level keeps what it
        // had.
        let numeric: Vec<f64> = history
            .get(&key)
            .expect("the history was just inserted")
            .iter()
            .filter_map(|e| match e.value {
                ProbEstimate::Value(v) => Some(v),
                ProbEstimate::NotObserved => None,
            })
            .collect();
        assert_eq!(
            numeric,
            Vec::<f64>::new(),
            "both estimates were not-observed"
        );
        assert_eq!(median(&numeric), None, "nothing to write means no write");

        // And the slot that is left over resolves to the default rather than
        // to a sentinel — which is the half that used to be wrong.
        let slot = crate::comparison::ProbabilitySlot::Unset;
        assert!(
            (slot.resolve(0.09) - 0.09).abs() < f64::EPSILON,
            "an unwritten slot must read as the level's default"
        );
        assert!(
            (slot.resolve(0.09) - crate::comparison::LEVEL_NOT_OBSERVED_VALUE).abs() > f64::EPSILON,
            "...and must NOT read as EM's in-session not-observed value"
        );
    }

    /// One mixed estimate is enough: the numeric one wins and the
    /// not-observed ones are ignored rather than dragging the median down.
    #[test]
    fn a_partly_observed_level_uses_only_the_numeric_estimates() {
        let estimates = [
            ProbEstimate::NotObserved,
            ProbEstimate::Value(0.4),
            ProbEstimate::NotObserved,
        ];
        let numeric: Vec<f64> = estimates
            .iter()
            .filter_map(|v| match v {
                ProbEstimate::Value(x) => Some(*x),
                ProbEstimate::NotObserved => None,
            })
            .collect();
        assert_eq!(median(&numeric), Some(0.4));
    }
}
