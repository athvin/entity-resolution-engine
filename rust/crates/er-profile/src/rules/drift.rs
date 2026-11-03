//! Rules that compare this profile against the previous one of the same corpus.
//!
//! Every other rule in the registry judges a single profile in isolation, which
//! is why they could ship before anything profiled automatically. These two
//! cannot: they need a series, and a series only exists because every delivery
//! now profiles itself as it lands.
//!
//! The comparison is deliberately narrow — presence and fill — because those
//! are the two changes that break matching silently. A cardinality shift
//! changes what a blocking key costs and shows up in tier 2's own numbers; a
//! column quietly arriving 40% emptier changes which records can be matched at
//! all, and nothing else in the product notices.

use crate::{Code, CorpusContext, CorpusRule, Finding, RuleInfo, Severity};
use serde_json::json;

/// How much of a column's reachable pairs a fill drop has to cost before P011
/// speaks, as a fraction.
///
/// Expressed in PAIRS rather than in percentage points of fill, because pairs
/// are what the drop actually costs and the relationship is quadratic: a rule
/// keyed on a column reaches `(filled / rows)²` of all pairs, so a column
/// falling from 95% to 90% fill — five points, which reads as minor — loses
/// **10%** of the pairs it could reach. A threshold in fill points would have to
/// be either so tight it fires on noise at high fill or so loose it misses that
/// case entirely; one in pairs is the same question the rest of the profile is
/// already asking.
const MIN_PAIR_LOSS: f64 = 0.10;

/// Fill below which a drop is not worth reporting as drift.
///
/// A column at 2% that falls to 1% has lost 75% of its reachable pairs, which
/// is true and useless: at that fill it could never block anything, and P002
/// has already said the column is mostly empty. Reporting it again as drift
/// would be the same problem twice in different words.
const MIN_PREVIOUS_FILL: f64 = 0.20;

const P011_GOOD: &str = "`email` was 94% filled in the previous profile and is 93% filled now — \
                         well inside the noise a delivery's composition creates.";
const P011_BAD: &str = "`email` fill fell from 95% to 71% since the previous profile, which \
                        costs 44% of the pairs a rule keyed on it could reach.";

pub struct FillRateDropped;

impl CorpusRule for FillRateDropped {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("P011"),
            title: "a column's fill rate dropped enough to cost real matching reach",
            explain: "A source that starts sending a field less often is the most common way a \
                      pipeline silently stops matching: nothing errors, every stage succeeds, and \
                      the only evidence is that fewer records find their pair. Note the altitude \
                      — a raw profile covers everything that source has ever landed, not just the \
                      newest batch, so this reports the CORPUS moving rather than one delivery's \
                      composition. That damps the noise a small delivery would otherwise create, \
                      and it means a fill rate that moves here has moved for good. This fires on the \
                      DROP only — a column filling UP is good news and needs no warning — and it \
                      states the loss in pairs rather than in percentage points of fill, because \
                      reach is quadratic in fill and five points at the top of the range is a \
                      tenth of the column's usefulness. It stays quiet below 20% previous fill, \
                      where P002 has already said the column is mostly empty and saying it again \
                      as drift would be the same problem twice. \
                      \
                      A source that stops sending a mapped column entirely lands HERE and not in \
                      P012, which is worth knowing because it is the opposite of what the names \
                      suggest: landing writes a payload key for every column the config maps, \
                      whether or not the file carried it, so the column goes on existing with \
                      nothing in it. Measured, not assumed — dropping a mapped column from a \
                      delivery produces this finding and never a column-set change.",
            conforming: P011_GOOD,
            violating: P011_BAD,
            severity: Severity::Warning,
        }
    }

    fn check(&self, ctx: &CorpusContext<'_>) -> Vec<Finding> {
        let Some(previous) = ctx.previous else {
            // No previous profile: this is the first of its corpus, and "has it
            // changed" has no answer. Silence is the answer, not "no change".
            return Vec::new();
        };
        let mut out = Vec::new();
        for (shape, _) in ctx.columns {
            let Some(before) = previous.column(&shape.column_name) else {
                continue; // appearing is P012's business, not this rule's
            };
            let (Some(was), Some(now)) = (before.fill_rate(), shape.fill_rate()) else {
                continue;
            };
            if was < MIN_PREVIOUS_FILL || now >= was {
                continue;
            }
            // The share of this column's reachable pairs that the drop costs.
            // Both endpoints must be non-null for a pair to be reachable, hence
            // the square on each side.
            let pair_loss = 1.0 - (now * now) / (was * was);
            if pair_loss < MIN_PAIR_LOSS {
                continue;
            }
            out.push(Finding::new(
                &self.info(),
                &shape.column_name,
                format!(
                    "`{}` fill fell from {:.0}% to {:.0}% since the previous profile ({}), which \
                     costs {:.0}% of the pairs a rule keyed on it could reach",
                    shape.column_name,
                    was * 100.0,
                    now * 100.0,
                    previous.captured_at,
                    pair_loss * 100.0
                ),
                json!({
                    "fill_before": was,
                    "fill_now": now,
                    "pair_loss": pair_loss,
                    "previous_captured_at": previous.captured_at,
                    "min_pair_loss": MIN_PAIR_LOSS,
                }),
            ));
        }
        out
    }
}

const P012_GOOD: &str = "(Nothing to report: the delivery carried the same columns as the one \
                         before it.)";
const P012_BAD: &str = "`phone_e164` was in the previous profile and is absent from this one — \
                        any comparison or blocking rule naming it now has nothing to read.";

pub struct ColumnSetChanged;

impl CorpusRule for ColumnSetChanged {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("P012"),
            title: "a column appeared or disappeared since the previous profile",
            explain: "What this does NOT fire on is the obvious guess, and the distinction was \
                      measured rather than reasoned about: a source that stops sending a mapped \
                      column still has that column in its profile, present and empty, because \
                      landing writes a payload key for every column the config maps. That case is \
                      P011. \
                      \
                      What reaches here is a change to the SHAPE the config declares — a mapping \
                      gaining or losing a column, or an entity's attribute set changing under the \
                      standardized and golden subjects. A disappearance is a warning because every \
                      comparison naming that attribute silently stops contributing and the corpus \
                      just resolves worse, with nothing failing. An appearance is `info`: not a \
                      problem, but a newly mapped column is the most likely thing to be worth \
                      checking, and nothing else in the product mentions it.",
            conforming: P012_GOOD,
            violating: P012_BAD,
            severity: Severity::Warning,
        }
    }

    fn check(&self, ctx: &CorpusContext<'_>) -> Vec<Finding> {
        let Some(previous) = ctx.previous else {
            return Vec::new();
        };
        let info = self.info();
        // `info` for an arrival, the rule's own severity for a departure. One
        // rule rather than two codes because they are one question — "did the
        // column set change" — and a reader comparing two deliveries wants both
        // halves of the answer together.
        let arrival = RuleInfo {
            severity: Severity::Info,
            ..info.clone()
        };
        let mut out = Vec::new();
        for (shape, _) in ctx.columns {
            if previous.column(&shape.column_name).is_none() {
                out.push(Finding::new(
                    &arrival,
                    &shape.column_name,
                    format!(
                        "`{}` is new since the previous profile ({}) — nothing maps it yet",
                        shape.column_name, previous.captured_at
                    ),
                    json!({ "change": "appeared", "previous_captured_at": previous.captured_at }),
                ));
            }
        }
        for before in &previous.columns {
            if !ctx
                .columns
                .iter()
                .any(|(shape, _)| shape.column_name == before.column_name)
            {
                out.push(Finding::new(
                    &info,
                    &before.column_name,
                    format!(
                        "`{}` was in the previous profile ({}) and is absent from this one — any \
                         comparison or blocking rule naming it now has nothing to read",
                        before.column_name, previous.captured_at
                    ),
                    json!({ "change": "disappeared", "previous_captured_at": previous.captured_at }),
                ));
            }
        }
        out
    }
}
