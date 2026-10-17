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
//! standardized column name) is checked here; the level SQL is proven by the
//! analyser + oracle on the generated side.

use std::collections::BTreeSet;

use er_config::{Config, EntityType};

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
}
