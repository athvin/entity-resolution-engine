//! P003, P004, P005, P007, P008 — how a column's values are spread.

use serde_json::json;

use crate::{Code, ColumnContext, ColumnRule, Finding, RuleInfo, Severity};

/// The mode share at which one value is reported as dominating.
///
/// A fifth, because that is roughly where a single block stops being a tail and
/// starts being the run: a value covering share `s` of `n` rows contributes
/// about `(s*n)^2 / 2` pairs on its own, so at s = 0.2 one value produces 2% of
/// the entire cartesian product by itself — more than a well-chosen key
/// generates in total.
const DOMINANT_SHARE: f64 = 0.20;

const P003_GOOD: &str = "`surname`: the most frequent value is `Jones` at 23 of 819 rows \
                         (2.8%) — a long tail, no single block worth worrying about.";
const P003_BAD: &str = "`addr_city`: the most frequent value covers 31% of rows. Blocking on \
                        this column would generate roughly 5% of the entire cartesian product \
                        from that one value.";

pub struct OneValueDominates;

impl ColumnRule for OneValueDominates {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("P003"),
            title: "no single value covers a large share of a column",
            explain: "Blocking cost is not driven by the average block, it is driven by the \
                      largest one. Pairs inside a block grow with its square, so a value \
                      covering share `s` of `n` rows contributes on the order of `(s*n)^2 / 2` \
                      comparisons by itself — which means one pathological value can dominate a \
                      run while the column's total pair count still looks affordable. The \
                      classic shape is a placeholder: every record with a blank-but-present \
                      postcode lands in one enormous block. The same skew also defeats match \
                      weighting unless term-frequency adjustment is switched on for the column, \
                      because agreeing on a value half the corpus shares is weak evidence being \
                      scored as though it were strong. Reported with the value and its share so \
                      the fix — exclude it, normalize it, or enable TF — can be chosen rather \
                      than guessed.",
            conforming: P003_GOOD,
            violating: P003_BAD,
            severity: Severity::Warning,
        }
    }

    fn check(&self, ctx: &ColumnContext<'_>) -> Vec<Finding> {
        // Needs values nobody may have counted. Returning nothing is the
        // honest answer; a share inferred from the shape alone would be made up.
        let Some(mode) = ctx.mode() else {
            return Vec::new();
        };
        if mode.share < DOMINANT_SHARE {
            return Vec::new();
        }
        let s = ctx.shape;
        let rows = mode.count as f64;
        let pairs = (rows * (rows - 1.0) / 2.0).max(0.0);
        vec![Finding::new(
            &self.info(),
            &s.column_name,
            format!(
                "`{}` is {:.0}% one value ({} rows), which alone generates about {} pairs \
                 if the column is used as a blocking key",
                s.column_name,
                mode.share * 100.0,
                mode.count,
                pairs as i64
            ),
            json!({
                "mode_share": mode.share,
                "mode_count": mode.count,
                "pairs_from_mode": pairs as i64,
                "threshold": DOMINANT_SHARE,
            }),
        )]
    }
}

/// Values that mean "absent" while occupying a non-empty cell.
///
/// Lower-cased and trimmed before comparison. Kept deliberately short: a long
/// list starts matching real data (`none` is a surname, `null` a place), and a
/// false positive here sends someone to clean a column that was already clean.
const PLACEHOLDERS: [&str; 12] = [
    "n/a",
    "na",
    "null",
    "none",
    "nil",
    "unknown",
    "unspecified",
    "not provided",
    "no data",
    "-",
    "--",
    "?",
];

/// The share at which a placeholder is worth reporting. Low, because a
/// placeholder is a correctness problem at any volume — it is a value that
/// compares equal to itself while meaning the opposite.
const PLACEHOLDER_SHARE: f64 = 0.01;

const P004_GOOD: &str = "`addr_region`: top values are `CA`, `TX`, `NY` — all real.";
const P004_BAD: &str = "`phone`: 4% of rows hold the literal `unknown`. Those records all \
                        compare equal to each other on phone and will be pulled into the same \
                        block and scored as agreeing.";

pub struct PlaceholderValuesPresent;

impl ColumnRule for PlaceholderValuesPresent {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("P004"),
            title: "a column holds no placeholder values standing in for absence",
            explain: "A placeholder is worse than a NULL, because it is a value: `unknown` \
                      equals `unknown`, so every record carrying it agrees with every other \
                      one. That turns absence into positive evidence of a match — the records \
                      block together, score as agreeing on the field, and the resulting cluster \
                      is wrong in a way no threshold can fix, because the model was told the \
                      truth about what it was given. A NULL, by contrast, is handled correctly \
                      everywhere: it fails the equality, lands on the null comparison level, \
                      and contributes nothing. So the remedy is almost always to map the \
                      placeholder to NULL at standardization rather than to tune anything. The \
                      threshold is deliberately low because this is a correctness problem at \
                      any volume, and the placeholder list is deliberately short because a \
                      longer one starts matching real surnames and place names. Graded `error` \
                      rather than `warning` because the consequence is a WRONG RESULT and not a \
                      cost: records sharing nothing but their missing data get merged, and an \
                      over-merge is the expensive mistake in entity resolution — far harder to \
                      notice, and far harder to undo, than a duplicate left behind.",
            conforming: P004_GOOD,
            violating: P004_BAD,
            severity: Severity::Error,
        }
    }

    fn check(&self, ctx: &ColumnContext<'_>) -> Vec<Finding> {
        let mut hits: Vec<(&str, i64, f64)> = Vec::new();
        for value in ctx.top_values {
            let Some(raw) = value.value.as_deref() else {
                continue;
            };
            let normalized = raw.trim().to_ascii_lowercase();
            if PLACEHOLDERS.contains(&normalized.as_str()) && value.share >= PLACEHOLDER_SHARE {
                hits.push((raw, value.count, value.share));
            }
        }
        if hits.is_empty() {
            return Vec::new();
        }
        let s = ctx.shape;
        let named = hits
            .iter()
            .map(|(v, _, share)| format!("`{v}` ({:.0}%)", share * 100.0))
            .collect::<Vec<_>>()
            .join(", ");
        vec![Finding::new(
            &self.info(),
            &s.column_name,
            format!(
                "`{}` holds placeholder values standing in for absence: {named} — these \
                 compare EQUAL to each other, so map them to NULL at standardization",
                s.column_name
            ),
            json!({
                "placeholders": hits
                    .iter()
                    .map(|(v, count, share)| json!({
                        "value": v, "count": count, "share": share
                    }))
                    .collect::<Vec<_>>(),
                "threshold": PLACEHOLDER_SHARE,
            }),
        )]
    }
}

/// The share of distinct values that normalization must collapse before it is
/// reported. A tenth: below that, case and spacing noise is incidental; above
/// it, the column is carrying the same entity under several spellings.
const NORMALIZE_COLLAPSE_SHARE: f64 = 0.10;

const P005_GOOD: &str = "`email`: 428 distinct raw, 428 distinct normalized — already clean.";
const P005_BAD: &str = "`addr_city`: 216 distinct raw, 180 distinct normalized. 36 of the \
                        distinct values (17%) differ from another only by case or spacing, so \
                        `New York`, `new york` and `New  York` are being treated as three \
                        different cities.";

pub struct NormalizationCollapsesValues;

impl ColumnRule for NormalizationCollapsesValues {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("P005"),
            title: "a column's values do not collapse substantially under normalization",
            explain: "This is the one finding a raw distinct count actively hides. A column \
                      holding `New York`, `new york` and `New  York` looks healthy — three \
                      distinct values, high cardinality, a promising blocking key — and is in \
                      fact one city spelled three ways. The consequence is a recall loss that \
                      nothing else reports: an exact-match blocking key separates the variants \
                      into different blocks, so those records are never compared, never scored, \
                      and never appear in a review queue. Comparing the distinct count before \
                      and after lowercasing, trimming and collapsing internal whitespace is \
                      what makes it visible, and it is cheap because both counts come from the \
                      same pass. The remedy is a standardization transform, not a looser \
                      comparator: normalizing once at ingest fixes blocking too, whereas a \
                      fuzzy comparison level only helps pairs that blocking already produced.",
            conforming: P005_GOOD,
            violating: P005_BAD,
            severity: Severity::Warning,
        }
    }

    fn check(&self, ctx: &ColumnContext<'_>) -> Vec<Finding> {
        let s = ctx.shape;
        // Both counts are the expensive exact ones; without them there is
        // nothing to compare and guessing from the sketch would be noise.
        let (Some(exact), Some(normalized)) = (s.distinct_exact, s.distinct_normalized) else {
            return Vec::new();
        };
        if exact == 0 || normalized >= exact {
            return Vec::new();
        }
        let collapsed = exact - normalized;
        let share = collapsed as f64 / exact as f64;
        if share < NORMALIZE_COLLAPSE_SHARE {
            return Vec::new();
        }
        vec![Finding::new(
            &self.info(),
            &s.column_name,
            format!(
                "`{}` has {exact} distinct values but only {normalized} once case and spacing \
                 are normalized — {collapsed} ({:.0}%) are variants of another, and an \
                 exact-match key puts them in different blocks",
                s.column_name,
                share * 100.0
            ),
            json!({
                "distinct_exact": exact,
                "distinct_normalized": normalized,
                "collapsed": collapsed,
                "collapsed_share": share,
                "threshold": NORMALIZE_COLLAPSE_SHARE,
            }),
        )]
    }
}

const P007_GOOD: &str = "`source_system`: 4 distinct values over 1000 rows — low cardinality, \
                         but it does discriminate.";
const P007_BAD: &str = "`country`: 1 distinct value over 1000 rows. Every record agrees, so a \
                        blocking key on it is the full cartesian product and a comparison on \
                        it contributes the same weight to every pair.";

pub struct ColumnIsConstant;

impl ColumnRule for ColumnIsConstant {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("P007"),
            title: "a column holds more than one distinct value",
            explain: "A constant column is the degenerate case of skew and deserves naming \
                      separately, because its two failure modes are both total rather than \
                      gradual. As a blocking key it is equivalent to no blocking at all: every \
                      record agrees, so the key produces the entire cartesian product, and on a \
                      corpus of any size that is the difference between a run and a runaway. As \
                      a comparison it contributes exactly the same weight to every pair, which \
                      shifts every score by a constant and changes no ranking — pure cost for \
                      zero discrimination. Neither shows up as an error anywhere: the run just \
                      gets slower or the model just gets less useful. Worth checking against \
                      the sampled flag too, since a narrow sample of a genuinely varied column \
                      can look constant.",
            conforming: P007_GOOD,
            violating: P007_BAD,
            severity: Severity::Warning,
        }
    }

    fn check(&self, ctx: &ColumnContext<'_>) -> Vec<Finding> {
        let s = ctx.shape;
        let populated = s.non_null_count - s.blank_count;
        // An empty column is P001's; a constant claim needs something present.
        if populated == 0 {
            return Vec::new();
        }
        // The exact count when we have it; the sketch is unreliable near 1 and
        // this rule's whole claim is about the value 1.
        let Some(distinct) = s.distinct_exact else {
            return Vec::new();
        };
        if distinct != 1 {
            return Vec::new();
        }
        vec![Finding::new(
            &self.info(),
            &s.column_name,
            format!(
                "`{}` holds a single distinct value across {populated} populated rows — as a \
                 blocking key it is the full cartesian product, as a comparison it discriminates \
                 nothing",
                s.column_name
            ),
            json!({ "distinct_exact": distinct, "populated": populated }),
        )]
    }
}

/// How close to unique a column must be before a collision in it is read as a
/// defect rather than as the distribution.
///
/// Calibrated against a false positive this rule actually produced: at 0.95 it
/// fired on a surname column holding 900 distinct values over 940 rows, which
/// is not a broken identifier but an ordinary high-cardinality field — and a
/// rule that speaks about every varied column makes the whole findings panel
/// not worth reading. At 0.99 the exceptions look like defects rather than like
/// variety.
///
/// The threshold is a stand-in for knowledge the profile does not yet have.
/// Which column is *supposed* to identify a record is declared in the tenant's
/// config (`record_id_column`), and once the findings pass can read it this rule
/// should check that column exactly, where ANY collision is a finding and no
/// threshold is needed at all.
const NEAR_UNIQUE_ABOVE: f64 = 0.99;

const P008_GOOD: &str = "`surname`: 900 distinct values over 940 rows (96%) — varied, but \
                         nobody expected a name to identify a record, so no collision is \
                         implied.";
const P008_BAD: &str = "`crm_id`: 997 distinct values over 1000 populated rows (99.7%). Three \
                         ids repeat, so the column cannot identify a record and whichever row \
                         lands last wins.";

pub struct IdentifierIsNotUnique;

impl ColumnRule for IdentifierIsNotUnique {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("P008"),
            title: "a near-unique column is actually unique",
            explain: "A column that is almost but not quite unique is the signature of a \
                      broken record identifier, and it is dangerous precisely because it looks \
                      fine: 997 distinct values in 1000 rows reads as a healthy identifier, and \
                      the three collisions silently decide which version of a record survives \
                      ingest. Graded `error` for that reason: the \
                      outcome is not a slower run or a missed duplicate but a row that is \
                      simply gone, overwritten by whichever copy landed last. Reported only when the EXACT distinct count was computed, never \
                      from the approximate one — `approx_count_distinct` is a HyperLogLog and is \
                      wrong in both directions (measured at 978 on a column of 1000 genuinely \
                      distinct ids), so inferring uniqueness from it would both invent \
                      collisions that do not exist and miss the handful that do. That is why \
                      `ColumnShape::is_unique` returns an Option and this rule stays silent \
                      rather than guessing: a false alarm here sends someone hunting duplicates \
                      in a clean column, and a missed one leaves records overwriting each other. \
                      The near-unique threshold is itself a stand-in for config: which column is \
                      MEANT to identify a record is declared, and once this pass can read that \
                      declaration the right check is an exact one on that column, where any \
                      collision at all is a finding and no threshold is needed.",
            conforming: P008_GOOD,
            violating: P008_BAD,
            severity: Severity::Error,
        }
    }

    fn check(&self, ctx: &ColumnContext<'_>) -> Vec<Finding> {
        let s = ctx.shape;
        // `is_unique` is None unless the exact count was computed.
        let Some(unique) = s.is_unique() else {
            return Vec::new();
        };
        if unique {
            return Vec::new();
        }
        let Some(distinct) = s.distinct_exact else {
            return Vec::new();
        };
        let populated = s.non_null_count;
        // Only interesting when the column LOOKS like an identifier. A surname
        // column is not unique and nobody expected it to be — and at a looser
        // threshold this rule fired on exactly that, which is what calibrated
        // NEAR_UNIQUE_ABOVE.
        let ratio = distinct as f64 / populated as f64;
        if ratio < NEAR_UNIQUE_ABOVE {
            return Vec::new();
        }
        vec![Finding::new(
            &self.info(),
            &s.column_name,
            format!(
                "`{}` looks like an identifier ({distinct} distinct over {populated} rows) but \
                 is not unique — {} value(s) repeat, so rows collide",
                s.column_name,
                populated - distinct
            ),
            json!({
                "distinct_exact": distinct,
                "non_null": populated,
                "collisions": populated - distinct,
                "cardinality_ratio": ratio,
            }),
        )]
    }
}
