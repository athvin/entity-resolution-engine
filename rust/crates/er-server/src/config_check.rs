//! Publish-time config analysis (design Part 3, Phase 5): before a config version
//! goes live, verify that its blocking and comparison definitions only reference
//! columns the standardized-records table actually has for the config's entity
//! type — so a mistyped or non-existent column is refused at publish rather than
//! failing a tenant's first run.
//!
//! Scope. A blocking rule's `expr` is a raw scalar SQL expression over
//! standardized columns, so it is parsed (via dedupe-core's sqlparser column
//! extractor, already on this binary's path — no DataFusion in the request lane)
//! and every column it references is checked against the `int_std_records` schema.
//! A comparison's *levels* are symbolic tokens whose SQL is produced from the
//! trained model (not available at publish), so only the comparison *key* (a
//! standardized column name) is checked here; the level SQL is model-derived
//! and governed by dedupe-core's execution-oracle golden fixtures (the captured
//! SQL/parquet baselines), not by the static analyser.

use std::collections::{BTreeMap, BTreeSet};

use er_config::{Config, EntityType, canonical_attributes};

/// What a profile says about one standardized column, as the advisories need it.
///
/// A narrow struct rather than the generated row, so the advisory logic is pure
/// and testable from literals — the same reason
/// [`config_reference_problems`] takes a `Config` and nothing else.
#[derive(Debug, Clone)]
pub struct ProfiledColumn {
    pub column_name: String,
    pub row_count: i64,
    pub non_null_count: i64,
    pub blank_count: i64,
    /// The most frequent value's share, when values were counted.
    pub top_share: Option<f64>,
    /// EXACT distinct values, when the profile computed them. `None` means the
    /// run used only the HyperLogLog sketch, and the duplicate floor declines
    /// to answer rather than derive a record count from an approximation that
    /// is wrong in both directions.
    pub distinct_exact: Option<i64>,
}

impl ProfiledColumn {
    /// Non-null, non-blank proportion. `None` for an empty corpus, where a fill
    /// rate has no meaning and `0.0` would assert one.
    fn fill_rate(&self) -> Option<f64> {
        if self.row_count == 0 {
            return None;
        }
        Some((self.non_null_count - self.blank_count) as f64 / self.row_count as f64)
    }
}

/// The share of a column's pairs lost to NULLs below which nothing is said.
///
/// Both endpoints of a pair must be present for an equality to hold, so a key
/// on a column that is 60% populated reaches only 36% of pairs. At 0.5 the
/// column still reaches half of them, which is a trade a tenant may be making
/// on purpose; above it, the rule is mostly not running.
const ADVISE_PAIR_LOSS_ABOVE: f64 = 0.5;

/// The dominant-value share above which a blocking key's cost is worth naming.
const ADVISE_MODE_SHARE_ABOVE: f64 = 0.2;

/// How nearly unique a compared attribute must be before rows sharing a value
/// are evidence of duplication rather than ordinary overlap.
///
/// Two filters, and BOTH are load-bearing — a measurement against the `base_10`
/// fixture is what proved it:
///
/// - **The config must compare on the column.** Cardinality alone cannot tell
///   an identifier from a timestamp. `updated_at_source` in that corpus holds
///   22 distinct values across 23 rows, which is as near-unique as a column
///   gets, and two records sharing an update time is a coincidence rather than
///   a duplicate. A first draft of this lived in the findings registry, which
///   is deliberately config-blind, and reported exactly that nonsense.
/// - **And the values must be mostly distinct.** `addr_postal` IS compared and
///   holds 11 values across 23 rows; neighbours are not duplicates.
const ADVISE_DUPLICATE_RATIO: f64 = 0.8;

/// What the latest profile says about the config being published — advice, never
/// a refusal.
///
/// **Why this is separate from [`config_reference_problems`] rather than more
/// of it.** That function's output refuses a publish, and it can, because a
/// column either exists or does not. Everything here is a judgement about data
/// that will be different next week: a column 97% empty today may be the one a
/// tenant is about to start populating, and refusing the publish would make the
/// profile a gate on work it cannot see. The findings registry draws the same
/// line — "recommendations, never silent changes" — and `er-compile` already
/// has the channel for it in `Compiled.notes`.
///
/// It closes a real hole even so. Until now the validator checked that a
/// blocking column EXISTS and stopped there, so a rule keyed on a column that
/// is empty, or whose single dominant value puts half the corpus in one block,
/// published cleanly and then destroyed either recall or runtime on the first
/// run — with nothing anywhere connecting the symptom to the config change that
/// caused it.
///
/// Returns an empty vec when nothing has profiled the corpus, which is the
/// honest answer: no profile means no advice, not "nothing is wrong".
#[must_use]
pub fn profile_advisories(config: &Config, profiled: &[ProfiledColumn]) -> Vec<String> {
    let by_name: BTreeMap<&str, &ProfiledColumn> = profiled
        .iter()
        .map(|c| (c.column_name.as_str(), c))
        .collect();
    let mut advisories = Vec::new();

    for rule in &config.blocking {
        // A rule whose SQL does not parse is already a refusal above; here it
        // simply has no columns to advise about.
        let Ok(refs) = dedupe_core::sql::columns::columns_used(&rule.expr, "blocking") else {
            continue;
        };
        for col in refs {
            let Some(stats) = by_name.get(col.column.as_str()) else {
                continue;
            };
            let Some(fill) = stats.fill_rate() else {
                continue;
            };
            // Both endpoints must be present, hence the square.
            let pair_loss = 1.0 - fill * fill;
            if pair_loss > ADVISE_PAIR_LOSS_ABOVE {
                advisories.push(format!(
                    "blocking key `{}` reads `{}`, which is {:.0}% populated — an equality is \
                     never true of a NULL, so this rule cannot reach {:.0}% of all pairs",
                    rule.key_type,
                    col.column,
                    fill * 100.0,
                    pair_loss * 100.0
                ));
            }
            if let Some(share) = stats.top_share
                && share > ADVISE_MODE_SHARE_ABOVE
            {
                // Pairs from one value are quadratic in its count, which is the
                // part that surprises people: one value on 20% of a 100k corpus
                // is 200 million comparisons from that block alone.
                //
                // The share's denominator is the column's NON-NULL rows, not all
                // of them — `dedupe_core::profile::top_values` divides by
                // `count(<col>)`. This multiplied it by `row_count` at first,
                // which overstates the block by 1/fill and the COMPARISONS by
                // 1/fill², because the count is squared: on a half-populated
                // column it exaggerated by 4x the one number the tenant is being
                // asked to act on. Thresholding on the share itself is right as
                // it stands — an equi-join only ever compares non-null values,
                // so a share of the populated rows IS the block's share.
                let rows = (share * stats.non_null_count as f64).round();
                // Below two rows there is no pair to report, and `rows - 1`
                // would go negative and print "-0 comparisons".
                if rows >= 2.0 {
                    let pairs = rows * (rows - 1.0) / 2.0;
                    advisories.push(format!(
                        "blocking key `{}` reads `{}`, whose most common value covers {:.0}% of \
                         its populated rows ({:.0} of them) — that one block alone is about \
                         {:.0} comparisons",
                        rule.key_type,
                        col.column,
                        share * 100.0,
                        rows,
                        pairs
                    ));
                }
            }
        }
    }

    // The duplicate floor: how much duplication is already visible, with no
    // model and no scored corpus. This is the question a tenant asks first, and
    // until now the product could not answer it until after the work the answer
    // was meant to justify.
    //
    // A FLOOR, and the wording says so: it counts rows sharing a value on a
    // compared, near-unique attribute. Duplicates that DISAGREE on that
    // attribute are invisible here, so the true count is usually higher.
    let mut floors: Vec<(&str, i64, i64, i64)> = Vec::new();
    for key in config.comparisons.keys() {
        let Some(stats) = by_name.get(key.as_str()) else {
            continue;
        };
        let Some(distinct) = stats.distinct_exact else {
            continue; // the sketch cannot support a record count
        };
        let present = stats.non_null_count - stats.blank_count;
        if present <= 0 || distinct <= 0 {
            continue;
        }
        if (distinct as f64) / (present as f64) < ADVISE_DUPLICATE_RATIO {
            continue;
        }
        let floor = present - distinct;
        if floor > 0 {
            floors.push((key.as_str(), floor, present, distinct));
        }
    }
    // The tightest bound, not the sum: a record repeating on two attributes
    // would be counted twice by a sum, which turns a lower bound into an
    // over-estimate and destroys the one property the number has.
    if let Some(&(column, floor, present, distinct)) =
        floors.iter().max_by_key(|(_, floor, _, _)| *floor)
    {
        advisories.push(format!(
            "at least {floor} records share an `{column}` with another record ({present} rows \
             carry {distinct} distinct values) — a floor measured with no model, so the real \
             count is usually higher"
        ));
    }

    advisories.extend(unmeasurable_exact_levels(config, &by_name));
    advisories
}

/// The `exact` levels whose u-probability this corpus cannot supply, at the
/// `u_max_pairs` the config declares.
///
/// **What the hazard is, and what it is not.** A level the u-sample never
/// observes is *not* scored at a sentinel. `estimate_u` records
/// `ProbEstimate::NotObserved`, and `populate_from_trained_values` filters
/// those out and leaves the slot untouched, so the level falls back to its
/// built-in default — which is the better behaviour and deliberate. (A design
/// note once concluded such a level scores at `u = 1e-6`; that was a stale doc
/// comment outliving the variant it described, and the claim is false. It is
/// worth naming here because it is the reason this advisory nearly got written
/// against the wrong hazard.)
///
/// The real cost is quieter: the tenant is told nothing when a level's
/// u-probability came from a prior rather than from their data. For most levels
/// the default is a considered position on the ladder, and that is fine. For an
/// `exact` level it can be off by orders of magnitude, because the true u of an
/// equality is the corpus's own collision probability — on a near-unique column,
/// about `1/distinct`.
///
/// **Only `exact` is predictable this way.** Every other token compares a
/// derived or fuzzy key (`jaro_winkler:`, `levenshtein:`, `first_n:`,
/// `username_exact`, `domain_exact`, `variant_match`, …) whose agreement rate
/// column-level statistics cannot bound, so this says nothing about them rather
/// than guessing.
///
/// **The arithmetic, and why it only ever errs quiet.** `estimate_u` joins its
/// sample to itself under `uid_l < uid_r`, so it counts UNORDERED pairs, and the
/// chunking partitions them rather than resampling. Over a column's `d` distinct
/// non-null values with counts `n_v` summing to `M`, out of `N` rows, the number
/// of agreeing pairs is `A/2` where `A = Σ n_v(n_v−1)`, and the chance a random
/// pair is one of them is `A / (N(N−1))`. Two bounds on `A` hold with nothing but
/// the profile, both via the mode count `n_max = top_share · M`:
///
/// - `A ≤ M(n_max − 1)` — no value repeats more often than the mode does
/// - `A ≥ M(M/d − 1)` — a uniform column minimises agreement
///
/// **`n_v(n_v−1)`, not `n_v²`.** A value appearing *once* contributes no agreeing
/// pair at all, and that is precisely the case this advisory exists for; the
/// square over-counts it by exactly the amount that matters. The first draft used
/// the square and predicted ten agreements on a perfectly unique column, which
/// cannot produce a single one.
///
/// Checked against real data rather than asserted: this closed form reproduces
/// all five `exact` u-probabilities in
/// `rust/fixtures/golden/fake_1000/model_after_u_simple.json` to **zero**
/// relative error — that golden runs unchunked with early stopping off, so it
/// examines every pair of the corpus — and both bounds hold on all five columns.
///
/// Only the UPPER bound decides anything here, so this fires only when no
/// distribution consistent with the profile could supply enough observations.
/// Early stopping and chunking can only reduce the pairs examined, which makes
/// under-measurement likelier, never less likely — so the estimate stays an
/// upper bound in practice too. A level that merely *might* go unmeasured stays
/// silent.
fn unmeasurable_exact_levels(
    config: &Config,
    by_name: &BTreeMap<&str, &ProfiledColumn>,
) -> Vec<String> {
    // The sampler's own early-stop target is the right threshold to borrow,
    // because that constant already encodes "enough observations for this
    // estimate to mean anything". Read from `UOptions` rather than restated, so
    // the two cannot drift.
    //
    // NOT `config.training.u_min_count_per_level`: `dedupe_core`'s `Linker`
    // builds `UOptions { max_pairs, ..Default::default() }`, so that config
    // field is never read and the effective minimum is always this default.
    let Some(min_count) =
        dedupe_core::training::estimate_u::UOptions::default().min_count_per_level
    else {
        return Vec::new();
    };
    let min_count = min_count as f64;
    let max_pairs = config.training.u_max_pairs as f64;
    let mut out = Vec::new();

    for (key, spec) in &config.comparisons {
        if !spec.levels.iter().any(|l| l.as_deref() == Some("exact")) {
            continue;
        }
        let Some(stats) = by_name.get(key.as_str()) else {
            continue;
        };
        let Some(distinct) = stats.distinct_exact else {
            continue;
        };
        // Without the mode's share there is no UPPER bound on the collision
        // probability — only the uniform LOWER bound — and firing on a lower
        // bound would claim a level cannot be measured when a concentrated
        // distribution would measure it easily.
        let Some(top_share) = stats.top_share else {
            continue;
        };
        let rows = stats.row_count as f64;
        let present = stats.non_null_count as f64;
        if rows < 2.0 || present <= 0.0 || distinct <= 0 || top_share <= 0.0 {
            continue;
        }

        let corpus_pairs = rows * (rows - 1.0) / 2.0;
        let examined = max_pairs.min(corpus_pairs);
        // A ≤ M(n_max − 1), clamped at zero: a mode of one means no value
        // repeats, so there is no agreeing pair to find at any sample size.
        // Rounded because it IS an integer: `top_values` stores the mode's row
        // count divided by the populated rows, so multiplying back recovers the
        // count, and rounding clears the float noise that would otherwise decide
        // whether a mode of exactly 1 reads as 1.0000000000000002.
        let mode_count = (top_share * present).round();
        let agreeing_at_most = if mode_count <= 1.0 {
            0.0
        } else {
            present * (mode_count - 1.0)
        };
        let collision_at_most = agreeing_at_most / (rows * (rows - 1.0));
        let expected_at_most = examined * collision_at_most;
        if expected_at_most >= min_count {
            continue;
        }

        let remedy = if collision_at_most <= 0.0 {
            // The sharpest form of the finding: not "the sample is too small"
            // but "there is nothing to sample". Naming a setting here would be
            // advice that cannot work.
            "no two records share a value on it at all, so no `u_max_pairs` can reach it — \
             its u stays at the default until the data repeats"
                .to_string()
        } else {
            // The pairs needed if the distribution is as concentrated as the
            // profile allows — a LOWER bound on the requirement, worded as one.
            let needed = min_count / collision_at_most;
            if needed > corpus_pairs {
                format!(
                    "the corpus holds only about {corpus_pairs:.0} pairs in total, so no \
                     `u_max_pairs` can reach it — its u stays at the default until the \
                     corpus grows"
                )
            } else {
                format!("reaching it would take a `u_max_pairs` of at least about {needed:.0}")
            }
        };
        out.push(format!(
            "comparison `{key}` declares an `exact` level, and at `u_max_pairs = {max_pairs:.0}` \
             the u-sample examines about {examined:.0} pairs — with {distinct} distinct values \
             over {present:.0} populated rows, an exact agreement is expected at most \
             {expected_at_most:.0} times, short of the {min_count:.0} observations the sampler \
             asks for. Its u-probability will fall back to a built-in default rather than being \
             measured on your data; {remedy}"
        ));
    }
    out
}

/// The problems found in a config's column references (empty = the config is
/// consistent with the standardized schema). Each is a human-readable reason the
/// publish should be refused.
#[must_use]
pub fn config_reference_problems(config: &Config) -> Vec<String> {
    let account = config.entity_type == EntityType::Account;
    let mut valid: BTreeSet<String> = er_schema::model::int_std_record_columns(account)
        .into_iter()
        .map(|c| c.name)
        .collect();
    // E11a: a borrowed column is a real column by the time the engine scores,
    // so a comparison may name it. Added BEFORE the blocking and comparison
    // checks below, or declaring a feature and then using it would be refused
    // at publish — which is the whole feature.
    for feature in &config.features.related {
        valid.extend(feature.columns());
    }

    let mut problems = Vec::new();

    // The related-record features themselves (E11a): the prefix has to be a
    // usable column name and the target source has to exist. Checked here so
    // the refusal is a 422 at publish rather than a SQL error inside a job an
    // hour later, naming a column the tenant never wrote.
    problems.extend(er_cli::related::validate(config));

    // The ensemble (E5b). Structural only: whether a name resolves to an ACTIVE
    // registry version is a Postgres question, checked by the publish handler so
    // the refusal is a 422 at publish rather than a failed job later.
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (i, spec) in config.models.iter().enumerate() {
        let name = spec.name.trim();
        if name.is_empty() {
            problems.push(format!("models[{i}] has an empty name"));
        } else if !seen.insert(name) {
            // A duplicate would score the same model twice and, worse, read as
            // though it were weighted — an ensemble is a SET of distinct models.
            problems.push(format!(
                "models[{i}] repeats `{name}`; each member must name a distinct model"
            ));
        }
        if name != spec.name {
            problems.push(format!(
                "models[{i}] name `{}` has leading or trailing whitespace; it must match a                  registry name exactly",
                spec.name
            ));
        }
    }

    // Blocking: each key expression must parse and reference only real columns.
    for rule in &config.blocking {
        match dedupe_core::sql::columns::columns_used(&rule.expr, "blocking") {
            Err(e) => problems.push(format!(
                "blocking key `{}` has invalid SQL (`{}`): {e}",
                rule.key_type, rule.expr
            )),
            Ok(refs) => {
                for col in refs {
                    if !valid.contains(&col.column) {
                        problems.push(format!(
                            "blocking key `{}` references unknown standardized column `{}`",
                            rule.key_type, col.column
                        ));
                    }
                }
            }
        }
    }

    // Comparison: the key is a standardized column name.
    for key in config.comparisons.keys() {
        if !valid.contains(key.as_str()) {
            problems.push(format!(
                "comparison column `{key}` is not a standardized column"
            ));
        }
    }

    // Constraints: each names a standardized column, which is compiled straight
    // into `l.<column> = r.<column>` conjuncts on every blocking rule. E3 added
    // the block and never added it here, so until now a misspelled constraint
    // column produced SQL referencing a column that does not exist and the run
    // failed — loudly, but minutes later and inside the tenant's pipeline rather
    // than at the publish that introduced it. `MatchConstraints::columns()` was
    // written for exactly this check and had no caller.
    for column in config.constraints.columns() {
        if !valid.contains(column) {
            problems.push(format!(
                "constraint column `{column}` is not a standardized column"
            ));
        }
    }

    // Transforms: keyed by CANONICAL attribute, not by standardized column —
    // they run on the raw value before standardization, so `given_name` is in
    // range and `name_variants` (which standardization computes) is not. A
    // misspelled key is otherwise completely silent: the rule simply never
    // applies, and the tenant concludes the feature is broken.
    let canonical: BTreeSet<&str> = canonical_attributes(config.entity_type)
        .iter()
        .copied()
        .collect();
    for (attribute, rules) in &config.transforms {
        if !canonical.contains(attribute.as_str()) {
            problems.push(format!(
                "transform on `{attribute}` is not a canonical attribute (one of: {})",
                canonical.iter().copied().collect::<Vec<_>>().join(", ")
            ));
        }
        if rules.is_empty() {
            problems.push(format!(
                "transform on `{attribute}` declares no rules, so it does nothing"
            ));
        }
        for rule in rules {
            match rule {
                er_config::TransformRule::RegexReplace { pattern, .. } => {
                    if pattern.is_empty() {
                        problems.push(format!(
                            "transform on `{attribute}`: regex_replace needs a pattern"
                        ));
                    } else if let Err(e) = regex_lite::Regex::new(pattern) {
                        // Checked at PUBLISH because the alternative is a
                        // standardize pass that fails for the whole corpus.
                        problems.push(format!(
                            "transform on `{attribute}`: regex_replace pattern `{pattern}` \
                             is invalid: {e}"
                        ));
                    }
                }
                er_config::TransformRule::Substring { start, length } => {
                    if *start < 1 || *length < 1 {
                        problems.push(format!(
                            "transform on `{attribute}`: substring is 1-based and needs a \
                             positive length, got start={start} length={length}"
                        ));
                    }
                }
                er_config::TransformRule::Strip { chars } if chars.is_empty() => {
                    problems.push(format!(
                        "transform on `{attribute}`: strip needs characters to remove"
                    ));
                }
                er_config::TransformRule::Dictionary { map } if map.is_empty() => {
                    problems.push(format!(
                        "transform on `{attribute}`: dictionary needs at least one mapping"
                    ));
                }
                _ => {}
            }
        }
    }

    // Segments: a predicate over the standardized columns, checked exactly like
    // a blocking expression. This is the only check a segment ever gets — a run
    // names a segment rather than supplying SQL, so if a bad predicate can be
    // published it will fail the tenant's run instead of their publish, with no
    // second chance to catch it.
    for (name, predicate) in &config.segments {
        match dedupe_core::sql::columns::columns_used(predicate, "segment") {
            Err(e) => problems.push(format!(
                "segment `{name}` has invalid SQL (`{predicate}`): {e}"
            )),
            Ok(refs) => {
                for col in refs {
                    if !valid.contains(&col.column) {
                        problems.push(format!(
                            "segment `{name}` references unknown standardized column `{}`",
                            col.column
                        ));
                    }
                }
            }
        }
    }

    // Survivorship groups (1.6c). Every one of these is a refusal rather than a
    // silent preference, because a group that half-applies is worse than no
    // group: the golden record looks consistent and is not.
    let scalar_vocabulary: &[&str] = if account {
        &er_golden::lineage::ACCT_SCALAR_ATTRIBUTES
    } else {
        &er_golden::lineage::SCALAR_ATTRIBUTES
    };
    let mut claimed_by: BTreeMap<&str, &str> = BTreeMap::new();
    for (token, group) in &config.survivorship_groups {
        if group.fields.len() < 2 {
            // A one-field group is a scalar wearing a group's name: it would
            // generate a lineage row under a token nothing else uses and change
            // no outcome. Saying so beats letting a tenant believe it grouped.
            problems.push(format!(
                "survivorship_groups `{token}` lists {} field(s); a group needs at least two, \
                 or it is just that field's own survivorship chain",
                group.fields.len()
            ));
        }
        if !config.survivorship.contains_key(token) {
            problems.push(format!(
                "survivorship_groups `{token}` has no survivorship chain; a group is decided \
                 once, under its own name — add `survivorship.{token}`"
            ));
        }
        for field in &group.fields {
            if !valid.contains(field) {
                problems.push(format!(
                    "survivorship_groups `{token}` references unknown standardized column \
                     `{field}`"
                ));
                continue;
            }
            if let Some(other) = claimed_by.insert(field.as_str(), token.as_str()) {
                problems.push(format!(
                    "`{field}` is in both survivorship_groups `{other}` and `{token}`; two \
                     decisions writing one column is the ambiguity groups exist to remove"
                ));
            }
            // A field in a group is decided BY the group, so its own chain would
            // be dead config — the shape this repo refuses outright rather than
            // honouring one of the two.
            if config.survivorship.contains_key(field) {
                problems.push(format!(
                    "`{field}` is in survivorship_groups `{token}` and also has its own \
                     `survivorship.{field}` chain; the group decides it, so that chain would \
                     never be read"
                ));
            }
        }
        if let Some(controlling) = &group.controlling
            && !group.fields.iter().any(|f| f == controlling)
        {
            problems.push(format!(
                "survivorship_groups `{token}` ranks on `{controlling}`, which is not one of its \
                 own fields"
            ));
        }
        // A token that is also an independently-elected attribute would generate
        // two relations with one name.
        if scalar_vocabulary.contains(&token.as_str()) && !group.fields.iter().any(|f| f == token) {
            problems.push(format!(
                "survivorship_groups `{token}` collides with the attribute `{token}`, which is \
                 still elected on its own; name the group something else or put `{token}` in it"
            ));
        }
    }

    problems.sort();
    problems.dedup();
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = include_str!("../../er-config/tests/fixtures/test.yaml");

    fn load(yaml: &str) -> Config {
        er_config::load_config_str(yaml).expect("config parses")
    }

    #[test]
    fn a_valid_config_has_no_reference_problems() {
        assert_eq!(config_reference_problems(&load(GOOD)), Vec::<String>::new());
    }

    /// E5b: the ensemble's structural rules. Whether a name resolves to an
    /// ACTIVE registry version is a Postgres question and lives in the publish
    /// handler; these are the ones a pure check can answer.
    #[test]
    fn an_ensemble_must_name_distinct_non_empty_models() {
        // A well-formed ensemble is silent.
        let good = load(&format!(
            "{GOOD}\nmodels:\n  - name: loose\n  - name: strict\n"
        ));
        assert_eq!(config_reference_problems(&good), Vec::<String>::new());

        // A repeat would score the same model twice and read as if it were
        // weighted; an ensemble is a set of distinct models.
        let dup = load(&format!(
            "{GOOD}\nmodels:\n  - name: loose\n  - name: strict\n  - name: loose\n"
        ));
        let problems = config_reference_problems(&dup);
        assert!(
            problems
                .iter()
                .any(|p| p.contains("models[2]") && p.contains("repeats `loose`")),
            "{problems:?}"
        );

        // An empty name matches no registry row, so it can only ever fail later.
        let empty = load(&format!("{GOOD}\nmodels:\n  - name: \"\"\n"));
        let problems = config_reference_problems(&empty);
        assert!(
            problems
                .iter()
                .any(|p| p.contains("models[0] has an empty name")),
            "{problems:?}"
        );

        // Whitespace is refused rather than trimmed: the name has to match a
        // registry name exactly, and silently trimming it would make the config
        // and the registry disagree about what the member is called.
        let padded = load(&format!("{GOOD}\nmodels:\n  - name: \" loose \"\n"));
        let problems = config_reference_problems(&padded);
        assert!(
            problems
                .iter()
                .any(|p| p.contains("models[0]") && p.contains("whitespace")),
            "{problems:?}"
        );
    }

    /// A segment predicate gets exactly one chance to be caught, and this is it:
    /// a run names a segment rather than supplying SQL, so a predicate that gets
    /// published will fail the tenant's run instead of their publish.
    #[test]
    fn a_segment_predicate_is_checked_like_a_blocking_expression() {
        let good = load(&format!(
            "{GOOD}\nsegments:\n  west: \"addr_region in ('CA','OR')\"\n"
        ));
        assert_eq!(config_reference_problems(&good), Vec::<String>::new());

        // An unknown column.
        let typo = load(&format!(
            "{GOOD}\nsegments:\n  west: \"addr_regoin = 'CA'\"\n"
        ));
        let problems = config_reference_problems(&typo);
        assert!(
            problems
                .iter()
                .any(|p| p.contains("segment `west`") && p.contains("`addr_regoin`")),
            "{problems:?}"
        );

        // SQL that does not parse at all.
        let broken = load(&format!(
            "{GOOD}\nsegments:\n  west: \"addr_region = = 'CA'\"\n"
        ));
        let problems = config_reference_problems(&broken);
        assert!(
            problems
                .iter()
                .any(|p| p.contains("segment `west`") && p.contains("invalid SQL")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_blocking_expr_referencing_an_unknown_column_is_flagged() {
        // `emale` is a typo for `email`.
        let bad = GOOD.replace(
            r#"{ key_type: email_exact, expr: "email" }"#,
            r#"{ key_type: email_exact, expr: "emale" }"#,
        );
        let problems = config_reference_problems(&load(&bad));
        assert!(
            problems
                .iter()
                .any(|p| p.contains("email_exact") && p.contains("`emale`")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_comparison_on_an_unknown_column_is_flagged() {
        // Add a comparison keyed on a column int_std_records does not have.
        let bad = GOOD.replace(
            "comparisons:",
            "comparisons:\n  not_a_column: { levels: [exact, null] }",
        );
        let problems = config_reference_problems(&load(&bad));
        assert!(
            problems
                .iter()
                .any(|p| p.contains("comparison column `not_a_column`")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_comparison_may_name_a_borrowed_column() {
        // THE point of E11a. Declaring a feature and then being refused for
        // using it would make the block decorative — and this is the assertion
        // that fails if the derived columns are added to `valid` after the
        // comparison check rather than before it.
        let with_feature = GOOD
            .replace(
                "comparisons:",
                "features:\n  related:\n    - prefix: employer\n      key_column: employer_id\n      target_source: crm\n      attributes: [given_name]\ncomparisons:\n  employer_given_name: { levels: [exact, null] }",
            );
        let problems = config_reference_problems(&load(&with_feature));
        assert!(
            !problems.iter().any(|p| p.contains("employer_given_name")),
            "a borrowed column must be nameable by a comparison: {problems:?}"
        );
    }

    #[test]
    fn a_feature_pointing_at_an_unknown_source_is_flagged() {
        // At publish, naming the alternatives. The run-time alternative is a
        // join against nothing, which produces NULLs rather than an error —
        // so the comparison silently never fires and the tenant concludes the
        // feature does not work.
        let bad = GOOD.replace(
            "comparisons:",
            "features:\n  related:\n    - prefix: employer\n      key_column: employer_id\n      target_source: nowhere\n      attributes: [given_name]\ncomparisons:",
        );
        let problems = config_reference_problems(&load(&bad));
        assert!(
            problems.iter().any(|p| p.contains("nowhere")),
            "{problems:?}"
        );
    }

    /// All three constraint kinds, because each is a separate list on the block
    /// and `columns()` chains them — a check that only walked one would pass a
    /// typo in the other two.
    #[test]
    fn a_constraint_on_an_unknown_column_is_flagged() {
        // A real column is accepted. `email` exists for both entity types.
        let good = load(&format!(
            "{GOOD}\nconstraints:\n  veto: [email]\n  required_match: [phone_e164]\n  \
             exclude_when_equal: [addr_postal]\n"
        ));
        assert_eq!(config_reference_problems(&good), Vec::<String>::new());

        for (block, column) in [
            ("veto", "ssn"),
            ("required_match", "tax_id"),
            ("exclude_when_equal", "household_id"),
        ] {
            // These three read like plausible constraint columns and are exactly
            // the ones the design prose uses as examples — but none of them is a
            // standardized column, so each must be refused at publish.
            let bad = load(&format!("{GOOD}\nconstraints:\n  {block}: [{column}]\n"));
            let problems = config_reference_problems(&bad);
            assert!(
                problems
                    .iter()
                    .any(|p| p.contains(&format!("constraint column `{column}`"))),
                "{block}: [{column}] must be refused, got {problems:?}"
            );
        }
    }

    /// `GOOD` with a group block appended, and the named chains removed from
    /// `survivorship:` so the fixture can declare a legal group too.
    fn with_group_block(block: &str, drop_chains: &[&str]) -> Config {
        let mut config = load(&format!("{GOOD}\n{block}"));
        for attr in drop_chains {
            config.survivorship.remove(*attr);
        }
        config
    }

    #[test]
    fn a_legal_survivorship_group_has_no_problems() {
        // The positive case first: a refusal list that rejects everything is
        // indistinguishable from one that is simply broken.
        let mut config = with_group_block(
            "survivorship_groups:\n  full_name:\n    fields: [given_name, family_name]\n    \
             controlling: family_name\n",
            &["given_name", "family_name"],
        );
        config
            .survivorship
            .insert("full_name".to_string(), vec!["recency".to_string()]);
        assert_eq!(config_reference_problems(&config), Vec::<String>::new());
    }

    #[test]
    fn every_survivorship_group_mistake_is_refused_at_publish() {
        // Each case is a config that half-works: the golden record would look
        // internally consistent and not be. Refused rather than preferred,
        // because a group that half-applies is worse than no group.
        let cases: [(&str, &[&str], &str); 6] = [
            (
                // A field that is not a standardized column at all.
                "survivorship_groups:\n  full_name:\n    fields: [given_name, surname]\n",
                &["given_name"],
                "unknown standardized column `surname`",
            ),
            (
                // One field: a scalar wearing a group's name.
                "survivorship_groups:\n  solo:\n    fields: [given_name]\n",
                &["given_name"],
                "a group needs at least two",
            ),
            (
                // No chain under the group's own name, so nothing decides it.
                "survivorship_groups:\n  full_name:\n    fields: [given_name, family_name]\n",
                &["given_name", "family_name"],
                "has no survivorship chain",
            ),
            (
                // Ranking on a field the group does not contain.
                "survivorship_groups:\n  full_name:\n    fields: [given_name, family_name]\n    \
                 controlling: email\n",
                &["given_name", "family_name"],
                "which is not one of its own fields",
            ),
            (
                // The grouped field keeps its own chain: dead config.
                "survivorship_groups:\n  full_name:\n    fields: [given_name, family_name]\n",
                &["family_name"],
                "would\n                     never be read",
            ),
            (
                // A token that is also still an independently-elected attribute,
                // which would generate two relations with one name.
                "survivorship_groups:\n  email:\n    fields: [given_name, family_name]\n",
                &["given_name", "family_name"],
                "collides with the attribute `email`",
            ),
        ];
        for (block, drop_chains, expected) in cases {
            let mut config = with_group_block(block, drop_chains);
            // Give every case the chain its group needs, except the case that is
            // about the missing chain.
            if !expected.contains("no survivorship chain") {
                let token = if block.contains("  solo:") {
                    "solo"
                } else if block.contains("  email:") {
                    "email"
                } else {
                    "full_name"
                };
                config
                    .survivorship
                    .entry(token.to_string())
                    .or_insert_with(|| vec!["recency".to_string()]);
            }
            let problems = config_reference_problems(&config);
            let needle: String = expected.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                problems.iter().any(|p| p
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .contains(&needle)),
                "{block:?} must be refused with {needle:?}, got {problems:?}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Publish-time advisories: what the profile says about this config.
    // -----------------------------------------------------------------------

    fn profiled(name: &str, rows: i64, present: i64, top_share: Option<f64>) -> ProfiledColumn {
        ProfiledColumn {
            column_name: name.to_string(),
            row_count: rows,
            non_null_count: present,
            blank_count: 0,
            top_share,
            distinct_exact: None,
        }
    }

    /// A column with an exact distinct count, which is what the duplicate floor
    /// is computed from.
    fn with_distinct(name: &str, present: i64, distinct: i64) -> ProfiledColumn {
        ProfiledColumn {
            column_name: name.to_string(),
            row_count: present,
            non_null_count: present,
            blank_count: 0,
            top_share: None,
            distinct_exact: Some(distinct),
        }
    }

    /// Which standardized columns the fixture config actually blocks on, so the
    /// tests below advise about real keys rather than invented ones.
    fn blocking_columns(config: &Config) -> Vec<String> {
        let mut out = Vec::new();
        for rule in &config.blocking {
            if let Ok(refs) = dedupe_core::sql::columns::columns_used(&rule.expr, "blocking") {
                out.extend(refs.into_iter().map(|c| c.column));
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    #[test]
    fn no_profile_means_no_advice_not_a_clean_bill() {
        // The distinction matters because the publish response shows these to a
        // human: an empty list has to mean "nothing measured this" and not
        // "measured, all fine", which is why the handler sends the list rather
        // than a boolean.
        assert_eq!(profile_advisories(&load(GOOD), &[]), Vec::<String>::new());
    }

    #[test]
    fn a_well_populated_blocking_column_is_silent() {
        let config = load(GOOD);
        let columns: Vec<ProfiledColumn> = blocking_columns(&config)
            .iter()
            .map(|name| profiled(name, 100_000, 99_000, Some(0.02)))
            .collect();
        assert_eq!(profile_advisories(&config, &columns), Vec::<String>::new());
    }

    #[test]
    fn an_empty_blocking_column_names_the_pair_loss() {
        // The hole this closes: `config_reference_problems` checks the column
        // EXISTS and stops, so a rule keyed on a 10%-populated column published
        // cleanly and then reached 1% of pairs.
        let config = load(GOOD);
        let first = blocking_columns(&config)
            .first()
            .expect("the fixture blocks on something")
            .clone();
        let columns = vec![profiled(&first, 100_000, 10_000, None)];
        let advisories = profile_advisories(&config, &columns);
        assert!(
            advisories
                .iter()
                .any(|a| a.contains(&first) && a.contains("99%")),
            "a 10% populated column loses 99% of pairs; got {advisories:?}"
        );
    }

    #[test]
    fn a_dominant_value_names_the_block_it_creates() {
        let config = load(GOOD);
        let first = blocking_columns(&config)
            .first()
            .expect("the fixture blocks on something")
            .clone();
        // Fully populated, so the NULL advisory stays quiet and only the skew
        // one speaks — otherwise this would pass on the wrong advisory.
        let columns = vec![profiled(&first, 100_000, 100_000, Some(0.30))];
        let advisories = profile_advisories(&config, &columns);
        assert_eq!(
            advisories.len(),
            1,
            "only the skew advisory: {advisories:?}"
        );
        assert!(
            advisories[0].contains("30%") && advisories[0].contains("comparisons"),
            "the advisory must name the share AND what it costs; got {advisories:?}"
        );
    }

    #[test]
    fn a_column_the_config_does_not_block_on_is_not_advised_about() {
        // A profile describes every column; only the ones a rule reads can
        // affect the run. Advising about the rest would bury the ones that
        // matter under a list nobody can act on.
        let config = load(GOOD);
        let columns = vec![profiled(
            "a_column_nothing_blocks_on",
            100_000,
            1,
            Some(0.99),
        )];
        assert_eq!(profile_advisories(&config, &columns), Vec::<String>::new());
    }

    #[test]
    fn an_empty_corpus_is_not_reported_as_an_empty_column() {
        // `row_count` of zero makes the fill rate undefined. Reporting 0% there
        // would advise a tenant to fix a column in a corpus that has no rows at
        // all, which is the ThresholdEditor's "0 of 0 pairs" mistake again.
        let config = load(GOOD);
        let first = blocking_columns(&config)
            .first()
            .expect("the fixture blocks on something")
            .clone();
        let columns = vec![profiled(&first, 0, 0, None)];
        assert_eq!(profile_advisories(&config, &columns), Vec::<String>::new());
    }

    // -----------------------------------------------------------------------
    // The duplicate floor — and the two filters a real corpus proved necessary.
    // -----------------------------------------------------------------------

    #[test]
    fn the_duplicate_floor_counts_rows_sharing_a_compared_near_unique_value() {
        // 7,412 rows carrying 7,326 distinct emails: 86 repeat. `email` is in
        // the fixture's `comparisons:`, so it is an attribute this tenant says
        // identifies an entity.
        let config = load(GOOD);
        let advisories = profile_advisories(&config, &[with_distinct("email", 7_412, 7_326)]);
        let floor = advisories
            .iter()
            .find(|a| a.contains("share an `email`"))
            .expect("the floor should be reported");
        assert!(floor.contains("at least 86 records"), "got {floor}");
        // It must read as a BOUND. A reader who takes it as the answer will
        // under-estimate, because duplicates that disagree on email are
        // invisible to it.
        assert!(
            floor.contains("a floor measured with no model") && floor.contains("usually higher"),
            "the bound must be stated as a bound: {floor}"
        );
    }

    #[test]
    fn a_near_unique_column_the_config_does_not_compare_is_ignored() {
        // THE case that moved this out of the findings registry.
        // `base_10`'s `updated_at_source` holds 22 distinct values across 23
        // rows — as near-unique as a column gets — and two records sharing an
        // update timestamp is a coincidence, not a duplicate. Cardinality alone
        // cannot tell an identifier from a clock, and the registry is
        // deliberately config-blind, so it reported exactly this nonsense.
        let config = load(GOOD);
        let advisories = profile_advisories(&config, &[with_distinct("updated_at_source", 23, 22)]);
        assert_eq!(
            advisories,
            Vec::<String>::new(),
            "a column outside `comparisons:` is not an identifier, got {advisories:?}"
        );
    }

    #[test]
    fn a_compared_but_low_cardinality_column_is_ignored() {
        // `addr_postal` IS compared, and in that same corpus holds 11 values
        // across 23 rows. Neighbours are not duplicates, so the cardinality
        // filter has to survive alongside the config one.
        let config = load(GOOD);
        let advisories = profile_advisories(&config, &[with_distinct("addr_postal", 23, 11)]);
        assert!(
            !advisories.iter().any(|a| a.contains("share an")),
            "sharing a postcode is not duplication, got {advisories:?}"
        );
    }

    #[test]
    fn the_floor_declines_to_answer_from_a_sketch() {
        // `distinct_exact: None` means the run used only the HyperLogLog.
        // Deriving a RECORD COUNT from a sketch that is wrong in both
        // directions would hand a tenant a number they read as exact.
        let config = load(GOOD);
        let advisories = profile_advisories(&config, &[profiled("email", 7_412, 7_412, None)]);
        assert!(
            !advisories.iter().any(|a| a.contains("share an")),
            "no exact distinct count means no floor, got {advisories:?}"
        );
    }

    #[test]
    fn the_floor_reports_the_tightest_bound_not_the_sum() {
        // 40 repeats on email, 150 on phone. The answer is 150, not 190: a
        // record repeating on both would be double-counted by a sum, turning a
        // lower bound into an over-estimate.
        let config = load(GOOD);
        let advisories = profile_advisories(
            &config,
            &[
                with_distinct("email", 1_000, 960),
                with_distinct("phone_e164", 1_000, 850),
            ],
        );
        let floor = advisories
            .iter()
            .find(|a| a.contains("share an"))
            .expect("one floor advisory");
        assert!(floor.contains("at least 150"), "got {floor}");
        assert!(
            floor.contains("phone_e164"),
            "and it must name the column: {floor}"
        );
        assert_eq!(
            advisories.iter().filter(|a| a.contains("share an")).count(),
            1,
            "one bound, not one per column"
        );
    }

    #[test]
    fn a_fully_unique_compared_column_says_nothing() {
        let config = load(GOOD);
        let advisories = profile_advisories(&config, &[with_distinct("email", 5_000, 5_000)]);
        assert!(
            !advisories.iter().any(|a| a.contains("share an")),
            "nothing repeats, so there is nothing to report: {advisories:?}"
        );
    }

    #[test]
    fn a_dominant_value_is_counted_over_populated_rows_not_all_rows() {
        // The regression this pins. `dedupe_core::profile::top_values` divides
        // by `count(<col>)`, so the share is of the column's POPULATED rows;
        // this multiplied it by `row_count`, overstating the block by 1/fill and
        // the comparisons by 1/fill² because the count is squared. Every earlier
        // test here used a fully populated column, where fill is 1 and the
        // mistake cannot show — which is exactly why it survived.
        let config = load(GOOD);
        // Half populated, mode on 40% of those: 20,000 rows, not 40,000.
        let columns = vec![profiled("email", 100_000, 50_000, Some(0.40))];
        let advisories = profile_advisories(&config, &columns);
        let skew = advisories
            .iter()
            .find(|a| a.contains("most common value"))
            .expect("a 40% mode is well over the threshold");
        assert!(
            skew.contains("20000"),
            "0.40 x 50,000 populated rows is 20,000 rows; got {skew}"
        );
        assert!(
            skew.contains("199990000"),
            "and 20,000 rows is 199,990,000 pairs — the quadratic is where the \
             old denominator showed up as a 4x exaggeration: got {skew}"
        );
    }

    // -----------------------------------------------------------------------
    // Which `exact` levels the u-sample can actually measure.
    // -----------------------------------------------------------------------

    /// A column carrying what the measurability bound needs: the exact distinct
    /// count, and how many rows the most frequent value occupies.
    ///
    /// `mode_count` is spelled as a count rather than the share the profile
    /// stores, because the count is the quantity the arithmetic turns on — the
    /// difference between a value appearing once (no agreeing pair) and twice
    /// (one) is the whole subject.
    fn measurability(
        name: &str,
        rows: i64,
        present: i64,
        distinct: i64,
        mode_count: i64,
    ) -> ProfiledColumn {
        ProfiledColumn {
            column_name: name.to_string(),
            row_count: rows,
            non_null_count: present,
            blank_count: 0,
            top_share: Some(mode_count as f64 / present as f64),
            distinct_exact: Some(distinct),
        }
    }

    #[test]
    fn an_exact_level_on_a_unique_column_can_never_be_observed() {
        // 100,000 distinct values over 100,000 rows: no two records share an
        // email, so the `exact` level has nothing to count at ANY sample size.
        // This is the case the first draft of the arithmetic got wrong — using
        // `n_v²` it predicted ten agreements on a column that cannot produce
        // one — and it is the sharpest form of the finding.
        let config = load(GOOD);
        let columns = vec![measurability("email", 100_000, 100_000, 100_000, 1)];
        let advisories = profile_advisories(&config, &columns);
        assert_eq!(advisories.len(), 1, "only the u advisory: {advisories:?}");
        let a = &advisories[0];
        assert!(
            a.contains("`email`") && a.contains("`exact`"),
            "it must name the comparison and the level: {a}"
        );
        assert!(
            a.contains("at most 0 times") && a.contains("100 observations"),
            "both numbers, so the gap is checkable rather than asserted: {a}"
        );
        assert!(
            a.contains("no two records share a value on it at all"),
            "and it must say a bigger setting cannot help: {a}"
        );
    }

    #[test]
    fn a_rarely_repeating_exact_level_names_the_setting_that_would_reach_it() {
        // 100,001 rows, the most frequent email on 3 of them: at most
        // 100,001 x 2 agreeing pairs among 100,001 x 100,000, so a million
        // sampled pairs expect at most 20 agreements against a target of 100.
        // Five million pairs would reach it.
        let config = load(GOOD);
        let columns = vec![measurability("email", 100_001, 100_001, 50_000, 3)];
        let advisories = profile_advisories(&config, &columns);
        assert_eq!(advisories.len(), 1, "only the u advisory: {advisories:?}");
        let a = &advisories[0];
        assert!(
            a.contains("at most 20 times"),
            "the expected observations: {a}"
        );
        assert!(
            a.contains("at least about 5000000"),
            "and the setting that would reach them: {a}"
        );
    }

    #[test]
    fn an_exact_level_the_sample_will_measure_is_silent() {
        // 1,000 values over 100,000 rows with a mode of 200: a million sampled
        // pairs expect ~1,990 agreements, comfortably past the target.
        let config = load(GOOD);
        let columns = vec![measurability("email", 100_000, 100_000, 1_000, 200)];
        assert_eq!(profile_advisories(&config, &columns), Vec::<String>::new());
    }

    #[test]
    fn a_comparison_with_no_exact_level_is_not_advised_about() {
        // Only `exact` has an agreement rate that column statistics can bound.
        // A fuzzy comparator's u depends on the comparator, so this has to stay
        // quiet rather than guess.
        const EXACT_PHONE: &str = "phone_e164:  { levels: [exact, null] }";
        assert_eq!(
            GOOD.matches(EXACT_PHONE).count(),
            1,
            "the fixture's phone comparison moved; this test edits it by text"
        );
        let fuzzy = GOOD.replace(
            EXACT_PHONE,
            "phone_e164:  { levels: [jaro_winkler:0.90, null] }",
        );
        let columns = vec![measurability("phone_e164", 100_000, 100_000, 100_000, 1)];

        // Positive control first: under the fixture's own `exact` level this
        // very column DOES fire. Without it, a typo in the column name would
        // make the silence below meaningless.
        assert_eq!(
            profile_advisories(&load(GOOD), &columns).len(),
            1,
            "the control must fire, or this test proves nothing"
        );
        assert_eq!(
            profile_advisories(&load(&fuzzy), &columns),
            Vec::<String>::new()
        );
    }

    #[test]
    fn without_a_mode_share_the_advisory_declines_to_claim() {
        // Without the mode there is no UPPER bound on agreement, only the
        // uniform lower one. Firing on a lower bound would claim a level cannot
        // be measured when a concentrated distribution would measure it easily,
        // so a profile that counted no values gets no advice.
        let config = load(GOOD);
        let columns = vec![with_distinct("email", 100_000, 100_000)];
        assert_eq!(profile_advisories(&config, &columns), Vec::<String>::new());
    }

    #[test]
    fn a_corpus_too_small_to_measure_says_so_rather_than_naming_a_setting() {
        // 150 rows with a mode of 2: 11,175 pairs exist in the whole corpus and
        // expect at most 75 agreements, but reaching 100 would take 14,900 — more
        // pairs than the corpus has. Naming a setting would be advice that cannot
        // be followed, however large it is.
        let config = load(GOOD);
        let columns = vec![measurability("email", 150, 150, 149, 2)];
        let advisories = profile_advisories(&config, &columns);
        let u = advisories
            .iter()
            .find(|a| a.contains("`exact`"))
            .expect("the u advisory must fire");
        assert!(
            u.contains("at most 75 times") && u.contains("no `u_max_pairs` can reach it"),
            "it must say the corpus is the limit, not the setting: {u}"
        );
        assert!(
            u.contains("11175"),
            "and name the pair count that bounds it: {u}"
        );
    }
}
