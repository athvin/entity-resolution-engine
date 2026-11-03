//! P009, P010 — whether this corpus can be blocked, and on what.

use serde_json::json;

use crate::{Code, CorpusContext, CorpusRule, Finding, RuleInfo, Severity};

/// The pair count above which blocking is worth discussing at all.
///
/// A row count would be the wrong unit. Blocking exists to defuse a QUADRATIC,
/// so the question "does this corpus need blocking?" is a question about pairs,
/// and the same `C(n, 2)` arithmetic the rest of this module uses answers it
/// directly.
///
/// Anchored to a measured datum rather than picked: the committed `fake_1000`
/// fixture is 1000 rows, so its unblocked corpus is 499,500 pairs, and it
/// resolves end to end in well under a second — half a million pairs is
/// demonstrably free. One order of magnitude above that is where the cost
/// starts to be worth a sentence, which puts the threshold at ~3,163 rows.
/// Below it the honest advice is to block on nothing, and saying "no column is
/// a good blocking key" would be true, useless, and the kind of warning that
/// teaches people to skip the panel.
const UNBLOCKED_PAIR_BUDGET: i64 = 5_000_000;

/// Whether this corpus is big enough for blocking to matter.
fn blocking_matters(rows: i64) -> bool {
    if rows < 2 {
        return false;
    }
    // f64 so a large corpus cannot overflow the product before the compare.
    let total = (rows as f64) * ((rows - 1) as f64) / 2.0;
    total > UNBLOCKED_PAIR_BUDGET as f64
}

/// The share of all possible comparisons a key must eliminate to count as
/// viable.
///
/// Blocking exists to turn a quadratic problem into a tractable one, so the
/// measure is the reduction ratio — the literature's term for exactly this —
/// and 99% is the point at which a key is doing that job rather than merely
/// trimming. A key that removes only 90% of comparisons leaves a tenth of a
/// quadratic, which on any corpus worth blocking is still quadratic.
const MIN_REDUCTION_RATIO: f64 = 0.99;

/// The share of pairs a key may lose to NULLs and still count as viable.
///
/// Half, because beyond that the key is buying its cheapness by declining to
/// look at most of the corpus — and the pairs it drops are not reported
/// anywhere else, which is what makes this worth pairing with the cost.
const MAX_PAIR_LOSS: f64 = 0.5;

/// A candidate's reduction ratio, or `None` when the corpus is too small for
/// the total to be meaningful.
fn reduction_ratio(expected_pairs: i64, rows: i64) -> Option<f64> {
    if rows < 2 {
        return None;
    }
    let total = (rows as f64) * ((rows - 1) as f64) / 2.0;
    Some(1.0 - (expected_pairs as f64) / total)
}

fn is_viable(c: &dedupe_core::blocking_analysis::CandidateKeyEstimate) -> bool {
    reduction_ratio(c.expected_pairs, c.rows).is_some_and(|r| r >= MIN_REDUCTION_RATIO)
        && c.pair_loss_from_nulls <= MAX_PAIR_LOSS
}

const P009_GOOD: &str = "`email` eliminates 99.6% of comparisons and loses 8% of pairs to \
                         NULLs — a usable key on its own.";
const P009_BAD: &str = "Across 12 measured columns the best key still leaves 14% of all \
                        possible comparisons, and the only selective column is 71% empty. \
                        No single column will block this corpus; it needs a composite key.";

pub struct NoViableBlockingKey;

impl CorpusRule for NoViableBlockingKey {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("P009"),
            title: "at least one column is a usable blocking key on its own",
            explain: "This is the one question a profile exists to answer before a model is \
                      trained, and it cannot be answered one column at a time: whether the \
                      corpus is blockable depends on the BEST column, so a per-column rule \
                      would either say nothing or say it twelve times. When no single column \
                      clears both bars the remedy is a composite key — `surname` plus \
                      `addr_postal` where neither alone is selective enough — and knowing that \
                      before the first run is the difference between a configuration decision \
                      and a three-hour surprise. Two bars rather than one because the failure \
                      modes are opposite: a key that eliminates too few comparisons makes the \
                      run quadratic, and a key that is cheap because its column is mostly empty \
                      makes the run miss most of its pairs. Silent when the candidates were not \
                      measured, because an empty candidate list means the tier did not run, not \
                      that nothing qualified.",
            conforming: P009_GOOD,
            violating: P009_BAD,
            severity: Severity::Warning,
        }
    }

    fn check(&self, ctx: &CorpusContext<'_>) -> Vec<Finding> {
        // Not measured is not the same as none viable.
        if ctx.candidate_keys.is_empty() {
            return Vec::new();
        }
        let rows = ctx.candidate_keys[0].rows;
        if !blocking_matters(rows) {
            return Vec::new();
        }
        if ctx.candidate_keys.iter().any(is_viable) {
            return Vec::new();
        }
        // Name the closest thing to a key there is, so the message is a starting
        // point rather than a dead end.
        let best = ctx
            .candidate_keys
            .iter()
            .min_by(|a, b| a.expected_pairs.cmp(&b.expected_pairs))
            .expect("non-empty");
        let best_reduction = reduction_ratio(best.expected_pairs, best.rows).unwrap_or(0.0);
        vec![Finding::new(
            &self.info(),
            "",
            format!(
                "no single column blocks this corpus of {rows} rows: the most selective is \
                 `{}`, which still leaves {:.1}% of all comparisons and loses {:.0}% of pairs \
                 to NULLs — consider a composite key",
                best.key_columns.join(" + "),
                (1.0 - best_reduction) * 100.0,
                best.pair_loss_from_nulls * 100.0
            ),
            json!({
                "rows": rows,
                "candidates_measured": ctx.candidate_keys.len(),
                "best_key": best.key_columns,
                "best_expected_pairs": best.expected_pairs,
                "best_reduction_ratio": best_reduction,
                "best_pair_loss_from_nulls": best.pair_loss_from_nulls,
                "min_reduction_ratio": MIN_REDUCTION_RATIO,
                "max_pair_loss": MAX_PAIR_LOSS,
            }),
        )]
    }
}

const P010_GOOD: &str = "(Nothing to report: this rule only speaks when a viable key exists, \
                         and what it says is which one.)";
const P010_BAD: &str = "`email` is the most selective usable key: 2,140,882 pairs, 99.6% of \
                        comparisons eliminated, 8% of pairs unreachable because the column is \
                        NULL.";

pub struct BestBlockingKey;

impl CorpusRule for BestBlockingKey {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("P010"),
            title: "the most selective usable blocking key is named with its cost",
            explain: "A findings panel that only ever reports problems is a panel people learn \
                      to dread and then to skip. This one carries the answer: of the columns \
                      measured, here is the key to block on and here is what it will cost. It is \
                      `info` rather than a warning because nothing is wrong — it exists so the \
                      most actionable line in the profile is present whether or not anything \
                      else fired. 'Most selective' is the lowest pair count among the keys that \
                      clear BOTH bars, never the lowest overall: the cheapest key on a corpus is \
                      very often a column that is almost entirely empty, and recommending that \
                      would be recommending a key that cannot see the data.",
            conforming: P010_GOOD,
            violating: P010_BAD,
            severity: Severity::Info,
        }
    }

    fn check(&self, ctx: &CorpusContext<'_>) -> Vec<Finding> {
        if ctx.candidate_keys.is_empty() {
            return Vec::new();
        }
        let rows = ctx.candidate_keys[0].rows;
        if !blocking_matters(rows) {
            return Vec::new();
        }
        let Some(best) = ctx
            .candidate_keys
            .iter()
            .filter(|c| is_viable(c))
            .min_by(|a, b| a.expected_pairs.cmp(&b.expected_pairs))
        else {
            return Vec::new();
        };
        let reduction = reduction_ratio(best.expected_pairs, best.rows).unwrap_or(0.0);
        vec![Finding::new(
            &self.info(),
            best.key_columns.join(" + "),
            format!(
                "`{}` is the most selective usable blocking key: {} pairs, {:.1}% of \
                 comparisons eliminated, {:.0}% of pairs unreachable to NULLs",
                best.key_columns.join(" + "),
                best.expected_pairs,
                reduction * 100.0,
                best.pair_loss_from_nulls * 100.0
            ),
            json!({
                "key": best.key_columns,
                "expected_pairs": best.expected_pairs,
                "reduction_ratio": reduction,
                "pair_loss_from_nulls": best.pair_loss_from_nulls,
                "largest_block_pairs": best.largest_block_pairs,
                "entropy_bits": best.entropy_bits,
                "distinct_values": best.distinct_values,
            }),
        )]
    }
}
