//! P001, P002 — how much of a column is actually there.

use serde_json::json;

use crate::{Code, ColumnContext, ColumnRule, Finding, RuleInfo, Severity};

/// A column with nothing in it at all.
const P001_GOOD: &str = "`email`: 1000 rows read, 940 populated — 94% full.";
const P001_BAD: &str = "`middle_name`: 1000 rows read, 0 populated. Every row is NULL or blank, so the column is \
     carrying no information and any comparison on it is pure cost.";

pub struct ColumnIsEmpty;

impl ColumnRule for ColumnIsEmpty {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("P001"),
            title: "a profiled column holds at least one value",
            explain: "An entirely empty column is reported separately from a mostly-empty one \
                      because the two call for different actions and because the arithmetic of \
                      the second hides the first: a fill rate of 0% and a fill rate of 3% both \
                      read as \"low\", but only one of them means the field is never populated \
                      and is safe to drop from the mapping entirely. It is also the single most \
                      common consequence of a mis-mapped column — a source column spelled \
                      slightly wrong produces a canonical attribute that is empty in every row, \
                      and nothing else in the pipeline will say so. Counting blank apart from \
                      NULL is what makes this detectable at all: ingest lands every delivered \
                      value as a JSON string, so an empty cell arrives as '' and a NULL-only \
                      check would call the column fully populated.",
            conforming: P001_GOOD,
            violating: P001_BAD,
            severity: Severity::Warning,
        }
    }

    fn check(&self, ctx: &ColumnContext<'_>) -> Vec<Finding> {
        let s = ctx.shape;
        if s.row_count == 0 {
            return Vec::new();
        }
        let populated = s.non_null_count - s.blank_count;
        if populated > 0 {
            return Vec::new();
        }
        vec![Finding::new(
            &self.info(),
            &s.column_name,
            format!(
                "`{}` is empty in all {} rows read ({} NULL, {} blank) — nothing maps into it",
                s.column_name,
                s.row_count,
                s.row_count - s.non_null_count,
                s.blank_count
            ),
            json!({
                "rows": s.row_count,
                "nulls": s.row_count - s.non_null_count,
                "blanks": s.blank_count,
            }),
        )]
    }
}

/// The threshold at which a column is reported as mostly empty.
///
/// Half, because that is the point at which a blocking key on the column
/// discards more pairs than it keeps: a key reaches only `fill^2` of all pairs,
/// so at a fill rate of 0.5 three quarters of the corpus's pairs are already
/// unreachable through it.
const MOSTLY_EMPTY_BELOW: f64 = 0.5;

const P002_GOOD: &str = "`phone_e164`: 1000 rows, 820 populated — 82% full, so a key on it \
                         still reaches about 67% of all pairs.";
const P002_BAD: &str = "`addr_postal`: 1000 rows, 410 populated — 41% full. A blocking key on \
                        this column can never reach 83% of the corpus's pairs, because both \
                        endpoints of a pair have to carry the value.";

pub struct ColumnIsMostlyEmpty;

impl ColumnRule for ColumnIsMostlyEmpty {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("P002"),
            title: "a column used for matching is populated in at least half its rows",
            explain: "Sparsity costs recall silently and quadratically. An equality is never \
                      true of a NULL, so a blocking key on a column with fill rate `f` reaches \
                      only `f^2` of the corpus's pairs — at 41% full that is 17%, and the other \
                      83% of pairs are not scored, not reviewed, and not reported anywhere. The \
                      square is the part people get wrong: a column that is \"only\" 40% empty \
                      feels tolerable and actually discards nearly two thirds of the pairs. The \
                      same sparsity also degrades training, because a comparison level that \
                      almost never fires accumulates too few observations to estimate from, and \
                      an unobserved level is read as a tiny probability rather than as an \
                      absence of evidence. This rule reports the fill rate and the implied pair \
                      loss together, because the second is the number that makes the first \
                      actionable.",
            conforming: P002_GOOD,
            violating: P002_BAD,
            severity: Severity::Warning,
        }
    }

    fn check(&self, ctx: &ColumnContext<'_>) -> Vec<Finding> {
        let s = ctx.shape;
        if s.row_count == 0 {
            return Vec::new();
        }
        let populated = s.non_null_count - s.blank_count;
        // An entirely empty column is P001's to report; saying it twice adds
        // nothing and buries the more specific finding.
        if populated == 0 {
            return Vec::new();
        }
        let fill = ctx.fill_rate();
        if fill >= MOSTLY_EMPTY_BELOW {
            return Vec::new();
        }
        let pair_loss = 1.0 - fill * fill;
        vec![Finding::new(
            &self.info(),
            &s.column_name,
            format!(
                "`{}` is {:.0}% populated ({} of {} rows), so a blocking key on it cannot \
                 reach {:.0}% of the corpus's pairs",
                s.column_name,
                fill * 100.0,
                populated,
                s.row_count,
                pair_loss * 100.0
            ),
            json!({
                "fill_rate": fill,
                "populated": populated,
                "rows": s.row_count,
                "pair_loss_from_nulls": pair_loss,
                "threshold": MOSTLY_EMPTY_BELOW,
            }),
        )]
    }
}
