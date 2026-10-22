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

use std::collections::BTreeSet;

use er_config::{Config, EntityType, canonical_attributes};

/// The problems found in a config's column references (empty = the config is
/// consistent with the standardized schema). Each is a human-readable reason the
/// publish should be refused.
#[must_use]
pub fn config_reference_problems(config: &Config) -> Vec<String> {
    let account = config.entity_type == EntityType::Account;
    let valid: BTreeSet<String> = er_schema::model::int_std_record_columns(account)
        .into_iter()
        .map(|c| c.name)
        .collect();

    let mut problems = Vec::new();

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
}
