//! Pure Fellegi-Sunter scoring math, log2-native.

use crate::comparison::{Comparison, ComparisonLevel, M_U_CLAMP_MIN};
use crate::error::{DedupeError, Result};

/// Per-level match weight: `log2(max(m, 1e-300) / max(u, 1e-300))`
/// (null levels contribute 0.0).
pub fn match_weight(m: f64, u: f64) -> f64 {
    (m.max(M_U_CLAMP_MIN) / u.max(M_U_CLAMP_MIN)).log2()
}

/// Prior as a log2 odds: `log2(lambda / (1 - lambda))`.
pub fn prior_log2_odds(lambda: f64) -> f64 {
    (lambda / (1.0 - lambda)).log2()
}

pub fn prob_to_match_weight(p: f64) -> f64 {
    prior_log2_odds(p)
}

pub fn match_weight_to_prob(w: f64) -> f64 {
    if w >= 0.0 {
        1.0 / (1.0 + 2f64.powf(-w))
    } else {
        let p = 2f64.powf(w);
        p / (1.0 + p)
    }
}

/// Resolve a level's (m, u), falling back to the per-position defaults
/// when unset.
pub fn level_m_u(_c: &Comparison, level: &ComparisonLevel) -> Result<(f64, f64)> {
    Ok((
        level.m.resolve(level.default_m),
        level.u.resolve(level.default_u),
    ))
}

/// The exact-match-level u probability: the u probability
/// the TF adjustment scales against. With `disable_tf_exact_match_detection`
/// it is the level's own u; otherwise the u of the sibling level that is an
/// exact match on exactly the TF column.
pub fn u_probability_for_tf_base(c: &Comparison, level: &ComparisonLevel) -> Result<f64> {
    let tf = level.tf.as_ref().expect("level has tf");
    if tf.disable_exact_match_detection {
        return level_m_u(c, level).map(|(_, u)| u);
    }
    let want = vec![tf.column.to_lowercase()];
    for sibling in &c.levels {
        if sibling.exact_match_columns.as_ref() == Some(&want) {
            return level_m_u(c, sibling).map(|(_, u)| u);
        }
    }
    Err(DedupeError::settings(format!(
        "comparison '{}': tf adjustment on level '{}' requires an exact-match level on column '{}' \
         (or disable_tf_exact_match_detection)",
        c.output_column_name, level.sql_condition, tf.column
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_weight_basics() {
        // log2(0.49063674384830286 / 0.006187690390473553) from the golden model.
        let w = match_weight(0.49063674384830286, 0.006187690390473553);
        assert!((w - 6.309110459952498).abs() < 1e-12);
        // u = 0 clamps instead of producing infinity.
        assert!(match_weight(0.5, 0.0).is_finite());
    }

    #[test]
    fn prior_odds() {
        let w = prior_log2_odds(0.00298012298012298);
        assert!((w - -8.386106589561935).abs() < 1e-12);
    }

    #[test]
    fn weight_prob_round_trip() {
        for &p in &[1e-9, 0.1, 0.5, 0.9, 1.0 - 1e-12] {
            let w = prob_to_match_weight(p);
            assert!((match_weight_to_prob(w) - p).abs() < 1e-9);
        }
        // Stability at extreme weights.
        assert_eq!(match_weight_to_prob(5000.0), 1.0);
        assert_eq!(match_weight_to_prob(-5000.0), 0.0);
    }
}
