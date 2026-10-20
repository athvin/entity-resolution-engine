//! 5-field cron expressions: parsing and "does this minute match".
//!
//! The scheduler is a **tick**, not a next-occurrence calculator: the dispatcher
//! asks once a minute whether each schedule matches the current minute. That
//! makes a matcher sufficient and keeps the semantics obvious — including the
//! deliberate absence of catch-up. A dispatcher that was down over a firing does
//! not backfill it when it returns, exactly like cron(8); backfilling would turn
//! an outage into a stampede of identical runs.
//!
//! Field order is `minute hour day-of-month month day-of-week`, with `*`,
//! `a`, `a-b`, `*/step`, `a-b/step` and comma-separated lists of those. Day-of-week
//! is `0-6` with `0` = Sunday, and `7` is accepted as Sunday too.
//!
//! The day rule follows cron(8): when **both** day-of-month and day-of-week are
//! restricted, a day matches if **either** does (not both). That surprises people,
//! so it is tested explicitly below.

/// One parsed field: the set of values it admits.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Field {
    /// Bit `i` set means value `min + i` matches.
    mask: u64,
    /// True when the field was written `*` (needed for the day-of-month /
    /// day-of-week "either" rule).
    wildcard: bool,
}

impl Field {
    fn matches(&self, value: u32, min: u32) -> bool {
        value
            .checked_sub(min)
            .is_some_and(|offset| offset < 64 && self.mask & (1u64 << offset) != 0)
    }
}

/// A parsed 5-field cron expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    minute: Field,
    hour: Field,
    day_of_month: Field,
    month: Field,
    day_of_week: Field,
}

/// Why an expression was rejected. Carries no source; the reporter formats it.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CronError {
    #[error(
        "a cron expression has 5 fields (minute hour day-of-month month day-of-week), got {got}"
    )]
    FieldCount { got: usize },
    #[error("field {field} ({value:?}) is not a valid cron field")]
    Field { field: &'static str, value: String },
    #[error("field {field} ({value:?}) is outside {min}..={max}")]
    Range {
        field: &'static str,
        value: String,
        min: u32,
        max: u32,
    },
    #[error("field {field} ({value:?}) has a step of zero")]
    ZeroStep { field: &'static str, value: String },
}

fn parse_field(spec: &str, field: &'static str, min: u32, max: u32) -> Result<Field, CronError> {
    let bad = || CronError::Field {
        field,
        value: spec.to_string(),
    };
    let out_of_range = || CronError::Range {
        field,
        value: spec.to_string(),
        min,
        max,
    };
    let mut mask = 0u64;
    let mut wildcard = false;
    for term in spec.split(',') {
        if term.is_empty() {
            return Err(bad());
        }
        let (range_part, step) = match term.split_once('/') {
            Some((r, s)) => {
                let step: u32 = s.parse().map_err(|_| bad())?;
                if step == 0 {
                    return Err(CronError::ZeroStep {
                        field,
                        value: spec.to_string(),
                    });
                }
                (r, step)
            }
            None => (term, 1),
        };
        let (lo, hi) = if range_part == "*" {
            wildcard = true;
            (min, max)
        } else if let Some((a, b)) = range_part.split_once('-') {
            let a: u32 = a.parse().map_err(|_| bad())?;
            let b: u32 = b.parse().map_err(|_| bad())?;
            if a > b {
                return Err(bad());
            }
            (a, b)
        } else {
            let v: u32 = range_part.parse().map_err(|_| bad())?;
            (v, v)
        };
        // Sunday is both 0 and 7 in cron; fold 7 down so the mask stays 0-6.
        let (lo, hi) = if field == "day-of-week" {
            (if lo == 7 { 0 } else { lo }, if hi == 7 { 0 } else { hi })
        } else {
            (lo, hi)
        };
        if lo < min || hi > max {
            return Err(out_of_range());
        }
        let mut v = lo;
        while v <= hi {
            mask |= 1u64 << (v - min);
            v += step;
        }
    }
    if mask == 0 {
        return Err(bad());
    }
    Ok(Field { mask, wildcard })
}

impl Cron {
    /// Parse a 5-field expression.
    ///
    /// # Errors
    /// Returns [`CronError`] when the field count, a term's syntax, a value's
    /// range or a step is invalid.
    pub fn parse(expr: &str) -> Result<Self, CronError> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(CronError::FieldCount { got: fields.len() });
        }
        Ok(Cron {
            minute: parse_field(fields[0], "minute", 0, 59)?,
            hour: parse_field(fields[1], "hour", 0, 23)?,
            day_of_month: parse_field(fields[2], "day-of-month", 1, 31)?,
            month: parse_field(fields[3], "month", 1, 12)?,
            day_of_week: parse_field(fields[4], "day-of-week", 0, 6)?,
        })
    }

    /// Does this expression fire during the given minute? `day_of_week` is
    /// 0 = Sunday.
    #[must_use]
    pub fn matches(
        &self,
        minute: u32,
        hour: u32,
        day_of_month: u32,
        month: u32,
        day_of_week: u32,
    ) -> bool {
        if !self.minute.matches(minute, 0)
            || !self.hour.matches(hour, 0)
            || !self.month.matches(month, 1)
        {
            return false;
        }
        let by_month_day = self.day_of_month.matches(day_of_month, 1);
        let by_week_day = self.day_of_week.matches(day_of_week, 0);
        // cron(8): both restricted -> either may satisfy; otherwise both must.
        match (self.day_of_month.wildcard, self.day_of_week.wildcard) {
            (false, false) => by_month_day || by_week_day,
            _ => by_month_day && by_week_day,
        }
    }

    /// Does this expression fire during `at`'s minute?
    #[must_use]
    pub fn matches_at(&self, at: chrono::DateTime<chrono::Utc>) -> bool {
        use chrono::{Datelike, Timelike};
        self.matches(
            at.minute(),
            at.hour(),
            at.day(),
            at.month(),
            at.weekday().num_days_from_sunday(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
            .unwrap()
            .and_utc()
    }

    #[test]
    fn every_minute_matches_always() {
        let c = Cron::parse("* * * * *").unwrap();
        assert!(c.matches_at(at("2026-10-20 13:37:00")));
        assert!(c.matches_at(at("2026-01-01 00:00:00")));
    }

    #[test]
    fn a_fixed_time_matches_only_that_minute() {
        let c = Cron::parse("30 3 * * *").unwrap();
        assert!(c.matches_at(at("2026-10-20 03:30:00")));
        assert!(!c.matches_at(at("2026-10-20 03:31:00")));
        assert!(!c.matches_at(at("2026-10-20 04:30:00")));
    }

    #[test]
    fn the_correction_pass_cadence_parses_and_fires_on_sunday() {
        // The cadence shipped in config.correction_pass: 03:00 on Sundays.
        let c = Cron::parse("0 3 * * 0").unwrap();
        assert!(
            c.matches_at(at("2026-10-18 03:00:00")),
            "2026-10-18 is a Sunday"
        );
        assert!(
            !c.matches_at(at("2026-10-19 03:00:00")),
            "Monday must not fire"
        );
        assert!(!c.matches_at(at("2026-10-18 04:00:00")));
    }

    #[test]
    fn steps_lists_and_ranges() {
        let c = Cron::parse("*/15 * * * *").unwrap();
        for m in [0, 15, 30, 45] {
            assert!(c.matches(m, 1, 1, 1, 1), "minute {m} should match */15");
        }
        assert!(!c.matches(7, 1, 1, 1, 1));

        let c = Cron::parse("0,30 9-17 * * 1-5").unwrap();
        assert!(c.matches(0, 9, 1, 1, 1), "Monday 09:00");
        assert!(c.matches(30, 17, 1, 1, 5), "Friday 17:30");
        assert!(!c.matches(15, 9, 1, 1, 1), "minute 15 is not in 0,30");
        assert!(!c.matches(0, 8, 1, 1, 1), "08:00 is below the hour range");
        assert!(!c.matches(0, 9, 1, 1, 6), "Saturday is outside 1-5");

        let c = Cron::parse("0 0 1-7/3 * *").unwrap();
        for d in [1, 4, 7] {
            assert!(c.matches(0, 0, d, 1, 3), "day {d} should match 1-7/3");
        }
        assert!(!c.matches(0, 0, 2, 1, 3));
    }

    #[test]
    fn sunday_is_both_zero_and_seven() {
        let zero = Cron::parse("0 3 * * 0").unwrap();
        let seven = Cron::parse("0 3 * * 7").unwrap();
        assert_eq!(zero, seven, "7 must fold to Sunday");
    }

    #[test]
    fn day_of_month_and_day_of_week_are_or_when_both_restricted() {
        // cron(8)'s surprising rule: the 1st OR a Monday, not the 1st that is a Monday.
        let c = Cron::parse("0 0 1 * 1").unwrap();
        assert!(
            c.matches(0, 0, 1, 10, 4),
            "the 1st fires even on a Thursday"
        );
        assert!(
            c.matches(0, 0, 19, 10, 1),
            "a Monday fires even on the 19th"
        );
        assert!(!c.matches(0, 0, 19, 10, 4), "neither -> no fire");

        // With one of them a wildcard the other is a plain restriction.
        let c = Cron::parse("0 0 * * 1").unwrap();
        assert!(c.matches(0, 0, 19, 10, 1));
        assert!(!c.matches(0, 0, 1, 10, 4));
    }

    #[test]
    fn invalid_expressions_are_rejected_with_a_reason() {
        assert_eq!(
            Cron::parse("* * * *").unwrap_err(),
            CronError::FieldCount { got: 4 }
        );
        assert_eq!(
            Cron::parse("* * * * * *").unwrap_err(),
            CronError::FieldCount { got: 6 }
        );
        // Out of range, which the old charset-only check accepted happily.
        assert!(matches!(
            Cron::parse("60 * * * *").unwrap_err(),
            CronError::Range {
                field: "minute",
                ..
            }
        ));
        assert!(matches!(
            Cron::parse("* 24 * * *").unwrap_err(),
            CronError::Range { field: "hour", .. }
        ));
        assert!(matches!(
            Cron::parse("* * 0 * *").unwrap_err(),
            CronError::Range {
                field: "day-of-month",
                ..
            }
        ));
        assert!(matches!(
            Cron::parse("* * * 13 *").unwrap_err(),
            CronError::Range { field: "month", .. }
        ));
        assert!(matches!(
            Cron::parse("* * * * 8").unwrap_err(),
            CronError::Range {
                field: "day-of-week",
                ..
            }
        ));
        assert!(matches!(
            Cron::parse("*/0 * * * *").unwrap_err(),
            CronError::ZeroStep {
                field: "minute",
                ..
            }
        ));
        for expr in ["x * * * *", "5-1 * * * *", ", * * * *", "- * * * *"] {
            assert!(Cron::parse(expr).is_err(), "{expr:?} should be rejected");
        }
    }

    #[test]
    fn february_29_only_matches_in_a_leap_year() {
        let c = Cron::parse("0 0 29 2 *").unwrap();
        assert!(c.matches_at(at("2028-02-29 00:00:00")));
        // 2026 has no Feb 29 at all, so the expression simply never fires that year.
        assert!(!c.matches_at(at("2026-02-28 00:00:00")));
    }
}
