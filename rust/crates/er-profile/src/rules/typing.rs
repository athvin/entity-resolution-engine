//! P006 — what a text column is actually holding.

use serde_json::json;

use crate::{Code, ColumnContext, ColumnRule, Finding, RuleInfo, Severity};

/// The share of values that must parse as one type before the column is
/// reported as that type in text form.
const TYPED_SHARE: f64 = 0.99;

const P006_GOOD: &str = "`given_name`: 0% parses as a date, 0% as a number — text holding text.";
const P006_BAD: &str = "`dob`: 1000 of 1000 values parse as a date, but the column is text and \
                        is compared with string equality, so `1990-01-02` and `1990-1-2` are \
                        different people.";

pub struct TextHoldsAConsistentType;

impl ColumnRule for TextHoldsAConsistentType {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("P006"),
            title: "a text column holding a single consistent type is compared as that type",
            explain: "Ingest stores every delivered value as text, so the declared type says \
                      nothing at all about content and type conformance is the only evidence of \
                      what a column actually holds. A column where every value parses as a date \
                      is a date being compared with string equality, and string equality on \
                      dates is wrong in a specific and silent way: `1990-01-02` and `1990-1-2` \
                      are the same day and different strings, while `02/01/1990` and `01/02/1990` \
                      are different days and equally plausible spellings. The same applies to \
                      numbers, where leading zeros and thousands separators decide equality. \
                      This is reported as information rather than a warning because text may be \
                      exactly right — an identifier that happens to be all digits must stay text \
                      or it loses its leading zeros — so the rule names what it found and leaves \
                      the choice of a date-aware comparator or a cast to the person who knows \
                      what the field means.",
            conforming: P006_GOOD,
            violating: P006_BAD,
            severity: Severity::Info,
        }
    }

    fn check(&self, ctx: &ColumnContext<'_>) -> Vec<Finding> {
        let s = ctx.shape;
        if s.non_null_count == 0 {
            return Vec::new();
        }
        let n = s.non_null_count as f64;
        // Date before timestamp, and both before the numeric kinds: a date
        // parses as a timestamp too, so reporting the narrowest match is the
        // informative one. Integers parse as doubles for the same reason.
        let candidates: [(&str, i64); 4] = [
            ("date", s.cast_date),
            ("timestamp", s.cast_timestamp),
            ("integer", s.cast_bigint),
            ("number", s.cast_double),
        ];
        let Some((kind, count)) = candidates
            .into_iter()
            .find(|(_, count)| (*count as f64) / n >= TYPED_SHARE)
        else {
            return Vec::new();
        };
        vec![Finding::new(
            &self.info(),
            &s.column_name,
            format!(
                "`{}` is text, but {count} of {} values parse as a {kind} — string equality on \
                 {kind}s treats different spellings of the same value as different values",
                s.column_name, s.non_null_count
            ),
            json!({
                "inferred_type": kind,
                "conforming": count,
                "non_null": s.non_null_count,
                "threshold": TYPED_SHARE,
                "cast_date": s.cast_date,
                "cast_timestamp": s.cast_timestamp,
                "cast_bigint": s.cast_bigint,
                "cast_double": s.cast_double,
            }),
        )]
    }
}
