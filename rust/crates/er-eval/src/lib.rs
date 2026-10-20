//! Pairwise quality metrics — the S8.5 contract (Rust port of
//! `src/er/eval/metrics.py`).
//!
//! Conventions, each pinned by a test and identical to the Python original:
//! - empty `predicted` yields precision `1.0` (nothing asserted, nothing wrong);
//! - empty `truth` yields recall `1.0` (nothing to miss);
//! - `f1` is `0.0` when `precision + recall == 0`;
//! - every pair is canonical `a < b` (S5.0); a non-canonical pair errors;
//! - metrics are evaluated strictly within `universe`; a predicted/truth pair
//!   outside it errors.
//!
//! Note vs Python: counts are `u64`, so the Python test that rejects
//! negative/bool/float counts is subsumed by the type system here.

use std::collections::{BTreeMap, BTreeSet};

/// A canonical pair: `a < b` (S5.0).
pub type Pair = (String, String);

#[derive(Debug, Clone, PartialEq)]
pub struct PairwiseMetrics {
    pub tp: u64,
    pub fp: u64,
    pub r#fn: u64,
    pub precision: f64,
    pub recall: f64,
    pub f1: f64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MetricsError {
    #[error(
        "{name} holds a non-canonical pair ({a:?}, {b:?}); S5.0 requires rec_a_key < rec_b_key"
    )]
    NonCanonical {
        name: &'static str,
        a: String,
        b: String,
    },
    #[error(
        "{count} predicted pair(s) lie outside the universe, e.g. {example:?}; S8.5 evaluates strictly within `universe`"
    )]
    PredictedOutsideUniverse { count: usize, example: Pair },
    #[error(
        "{count} truth pair(s) lie outside the universe, e.g. {example:?}; restrict the truth to the universe the prediction was made over (S8.5)"
    )]
    TruthOutsideUniverse { count: usize, example: Pair },
    #[error(
        "{key:?} appears under two entities ({first:?}, {second:?}); entity_membership holds one row per record (S4.5.3)"
    )]
    DuplicateMembership {
        key: String,
        first: String,
        second: String,
    },
}

/// The ratio conventions, over already-counted pair sets (S8.5).
#[must_use]
pub fn pairwise_metrics_from_counts(tp: u64, fp: u64, r#fn: u64) -> PairwiseMetrics {
    let precision = if tp + fp == 0 {
        1.0
    } else {
        tp as f64 / (tp + fp) as f64
    };
    let recall = if tp + r#fn == 0 {
        1.0
    } else {
        tp as f64 / (tp + r#fn) as f64
    };
    let f1 = if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    };
    PairwiseMetrics {
        tp,
        fp,
        r#fn,
        precision,
        recall,
        f1,
    }
}

fn require_canonical(name: &'static str, pairs: &BTreeSet<Pair>) -> Result<(), MetricsError> {
    for (a, b) in pairs {
        if a >= b {
            return Err(MetricsError::NonCanonical {
                name,
                a: a.clone(),
                b: b.clone(),
            });
        }
    }
    Ok(())
}

/// Precision/recall/f1 + raw counts, evaluated strictly within `universe`.
///
/// `BTreeSet::difference` yields in sorted order, so the "e.g." example in an
/// out-of-universe error is the lexicographically smallest stray — matching the
/// Python `sorted(stray)[0]`.
pub fn pairwise_metrics(
    predicted: &BTreeSet<Pair>,
    truth: &BTreeSet<Pair>,
    universe: &BTreeSet<Pair>,
) -> Result<PairwiseMetrics, MetricsError> {
    require_canonical("predicted", predicted)?;
    require_canonical("truth", truth)?;
    require_canonical("universe", universe)?;

    let stray_predicted: Vec<&Pair> = predicted.difference(universe).collect();
    if let Some(example) = stray_predicted.first() {
        return Err(MetricsError::PredictedOutsideUniverse {
            count: stray_predicted.len(),
            example: (*example).clone(),
        });
    }
    let stray_truth: Vec<&Pair> = truth.difference(universe).collect();
    if let Some(example) = stray_truth.first() {
        return Err(MetricsError::TruthOutsideUniverse {
            count: stray_truth.len(),
            example: (*example).clone(),
        });
    }

    let tp = predicted.intersection(truth).count() as u64;
    let fp = predicted.difference(truth).count() as u64;
    let r#fn = truth.difference(predicted).count() as u64;
    Ok(pairwise_metrics_from_counts(tp, fp, r#fn))
}

/// `entity_id -> members` from `(record_key, entity_id)` rows; a record under
/// two entities errors (entity_membership is one row per record, S4.5.3).
pub fn membership_partition(
    rows: &[(String, String)],
) -> Result<BTreeMap<String, BTreeSet<String>>, MetricsError> {
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    let mut grouped: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (record_key, entity_id) in rows {
        if let Some(prev) = seen.get(record_key)
            && prev != entity_id
        {
            return Err(MetricsError::DuplicateMembership {
                key: record_key.clone(),
                first: prev.clone(),
                second: entity_id.clone(),
            });
        }
        seen.insert(record_key.clone(), entity_id.clone());
        grouped
            .entry(entity_id.clone())
            .or_default()
            .insert(record_key.clone());
    }
    Ok(grouped)
}

/// The transitive closure of a membership as canonical within-entity pairs (S8.5).
pub fn cluster_closure_pairs(rows: &[(String, String)]) -> Result<BTreeSet<Pair>, MetricsError> {
    let mut pairs = BTreeSet::new();
    for members in membership_partition(rows)?.values() {
        let ordered: Vec<&String> = members.iter().collect(); // BTreeSet is already sorted
        for i in 0..ordered.len() {
            for j in (i + 1)..ordered.len() {
                pairs.insert((ordered[i].clone(), ordered[j].clone()));
            }
        }
    }
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(pairs: &[(&str, &str)]) -> BTreeSet<Pair> {
        pairs
            .iter()
            .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
            .collect()
    }
    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    // src/er/eval/metrics.py test: test_hand_computed_case (tp=fp=fn=1 -> 0.5).
    #[test]
    fn hand_computed_case() {
        let m = pairwise_metrics(
            &set(&[("s:a", "s:b"), ("s:a", "s:c")]),
            &set(&[("s:a", "s:b"), ("s:b", "s:c")]),
            &set(&[("s:a", "s:b"), ("s:a", "s:c"), ("s:b", "s:c")]),
        )
        .unwrap();
        assert_eq!((m.tp, m.fp, m.r#fn), (1, 1, 1));
        assert!(approx(m.precision, 0.5) && approx(m.recall, 0.5) && approx(m.f1, 0.5));
    }

    // test_degenerate_inputs: the three vacuity conventions.
    #[test]
    fn degenerate_inputs() {
        let u = set(&[("s:a", "s:b"), ("s:a", "s:c"), ("s:b", "s:c")]);
        let np = pairwise_metrics(&BTreeSet::new(), &set(&[("s:a", "s:b")]), &u).unwrap();
        assert_eq!((np.tp, np.fp, np.r#fn), (0, 0, 1));
        assert!(approx(np.precision, 1.0) && approx(np.recall, 0.0) && approx(np.f1, 0.0));

        let nt = pairwise_metrics(&set(&[("s:a", "s:b")]), &BTreeSet::new(), &u).unwrap();
        assert_eq!((nt.tp, nt.fp, nt.r#fn), (0, 1, 0));
        assert!(approx(nt.precision, 0.0) && approx(nt.recall, 1.0) && approx(nt.f1, 0.0));

        let be = pairwise_metrics(&BTreeSet::new(), &BTreeSet::new(), &BTreeSet::new()).unwrap();
        assert!(approx(be.precision, 1.0) && approx(be.recall, 1.0) && approx(be.f1, 1.0));
    }

    // test_partition_shapes: all-singletons vs all-in-one over 5 records / 4 true pairs.
    #[test]
    fn partition_shapes() {
        let keys: Vec<String> = (0..5).map(|i| format!("s:{i}")).collect();
        let mut universe = BTreeSet::new();
        for i in 0..5 {
            for j in (i + 1)..5 {
                universe.insert((keys[i].clone(), keys[j].clone()));
            }
        }
        let mut truth = BTreeSet::new();
        for i in 0..3 {
            for j in (i + 1)..3 {
                truth.insert((keys[i].clone(), keys[j].clone()));
            }
        }
        truth.insert((keys[3].clone(), keys[4].clone()));
        assert_eq!(truth.len(), 4);

        let s = pairwise_metrics(&BTreeSet::new(), &truth, &universe).unwrap();
        assert_eq!((s.tp, s.fp, s.r#fn), (0, 0, 4));
        assert!(approx(s.precision, 1.0) && approx(s.recall, 0.0));

        let one = pairwise_metrics(&universe, &truth, &universe).unwrap();
        assert_eq!((one.tp, one.fp, one.r#fn), (4, 6, 0));
        assert!(approx(one.precision, 0.4) && approx(one.recall, 1.0) && approx(one.f1, 0.8 / 1.4));
    }

    // test_out_of_universe_and_non_canonical_raise.
    #[test]
    fn out_of_universe_and_non_canonical_raise() {
        let u = set(&[("s:a", "s:b"), ("s:a", "s:c"), ("s:b", "s:c")]);
        assert!(matches!(
            pairwise_metrics(&set(&[("s:a", "s:z")]), &BTreeSet::new(), &u),
            Err(MetricsError::PredictedOutsideUniverse { .. })
        ));
        assert!(matches!(
            pairwise_metrics(&BTreeSet::new(), &set(&[("s:a", "s:z")]), &u),
            Err(MetricsError::TruthOutsideUniverse { .. })
        ));
        assert!(matches!(
            pairwise_metrics(&u, &BTreeSet::new(), &set(&[("s:a", "s:b")])),
            Err(MetricsError::PredictedOutsideUniverse { .. })
        ));
        assert!(matches!(
            pairwise_metrics(&set(&[("s:b", "s:a")]), &BTreeSet::new(), &u),
            Err(MetricsError::NonCanonical { .. })
        ));
        assert!(matches!(
            pairwise_metrics(&BTreeSet::new(), &set(&[("s:b", "s:a")]), &u),
            Err(MetricsError::NonCanonical { .. })
        ));
        assert!(matches!(
            pairwise_metrics(&BTreeSet::new(), &BTreeSet::new(), &set(&[("s:a", "s:a")])),
            Err(MetricsError::NonCanonical { .. })
        ));
    }

    // test_cluster_closure_pairs_is_canonical_and_transitive.
    #[test]
    fn cluster_closure_is_canonical_and_transitive() {
        let rows: Vec<(String, String)> = [
            ("s:c", "E1"),
            ("s:a", "E1"),
            ("s:b", "E1"),
            ("s:z", "E2"),
            ("s:y", "E2"),
        ]
        .iter()
        .map(|(k, e)| ((*k).to_string(), (*e).to_string()))
        .collect();

        let closure = cluster_closure_pairs(&rows).unwrap();
        assert_eq!(
            closure,
            set(&[
                ("s:a", "s:b"),
                ("s:a", "s:c"),
                ("s:b", "s:c"),
                ("s:y", "s:z")
            ])
        );
        assert!(closure.iter().all(|(a, b)| a < b));

        let part = membership_partition(&rows).unwrap();
        let e1: BTreeSet<String> = ["s:a", "s:b", "s:c"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let e2: BTreeSet<String> = ["s:y", "s:z"].iter().map(|s| (*s).to_string()).collect();
        assert_eq!(part.get("E1"), Some(&e1));
        assert_eq!(part.get("E2"), Some(&e2));

        assert!(matches!(
            membership_partition(&[
                ("s:a".to_string(), "E1".to_string()),
                ("s:a".to_string(), "E2".to_string())
            ]),
            Err(MetricsError::DuplicateMembership { .. })
        ));
        // The same assignment twice is a re-statement, not a conflict.
        assert_eq!(
            cluster_closure_pairs(&[
                ("s:a".to_string(), "E1".to_string()),
                ("s:a".to_string(), "E1".to_string())
            ])
            .unwrap(),
            BTreeSet::new()
        );
    }
}
