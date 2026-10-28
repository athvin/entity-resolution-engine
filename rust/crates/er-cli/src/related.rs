//! Borrowing attributes from a related record (E11a).
//!
//! Two "J. Smith" records are indistinguishable until you notice one works at
//! Acme and the other does not. The employer is not on either record — it is on
//! the record their `employer_id` points at — so no comparison can reach it and
//! the pair sits in the gray band forever.
//!
//! # Why this is a join here and not a change to the engine
//!
//! The roadmap assumed `features.related:` had to be "compiled into the concat
//! stage", which would have meant editing `dedupe_core`'s SQL — and four golden
//! suites read through that SQL, so it carried a determinism obligation nobody
//! wanted to pay.
//!
//! It does not have to. `concat_sql_single` projects `input.columns`, so
//! widening the standardized table BEFORE it is registered as the engine's
//! input carries the borrowed columns through untouched. `dedupe-core` is not
//! edited at all, the goldens cannot move, and a config with no `related:`
//! block registers exactly the table it registers today — which is the
//! determinism property, obtained structurally rather than asserted.

use er_config::{Config, RelatedFeature};

/// The standardized columns a borrowed attribute may name.
///
/// A key or attribute outside this set is read out of `metadata` instead, which
/// is where every unmapped source column already lands — and a foreign key
/// almost never IS a canonical attribute, so that is the common path rather
/// than the exception.
fn is_standard_column(name: &str) -> bool {
    const STANDARD: [&str; 16] = [
        "record_key",
        "source_system",
        "source_record_id",
        "given_name",
        "family_name",
        "name_variants",
        "email",
        "email_valid",
        "phone_e164",
        "phone_valid",
        "addr_number",
        "addr_street",
        "addr_unit",
        "addr_city",
        "addr_region",
        "addr_postal",
    ];
    STANDARD.contains(&name)
}

/// A SQL string literal, single quotes doubled.
fn lit(value: &str) -> String {
    value.replace('\'', "''")
}

/// How one column is read off a standardized row.
///
/// `metadata ->> 'x'` rather than a cast: the operator yields text, which is
/// what every borrowed column is compared as, and a typed extraction would make
/// the comparison's behaviour depend on how a source happened to encode a
/// number.
///
/// **Parenthesised deliberately.** The result is interpolated into a larger
/// expression — notably concatenated into the join predicate below — and DuckDB
/// 2.0 reordered `->>` relative to `||`. Unparenthesised,
/// `'crm' || ':' || s.metadata ->> 'employer_id'` re-associated to
/// `('crm' || ':' || s.metadata) ->> 'employer_id'`, which asks DuckDB to parse
/// the string `crm:{}` as JSON. It failed loudly here, but a predicate that
/// merely stopped matching would not have, so the parentheses stay regardless
/// of what any engine's precedence table says.
fn read_column(alias: &str, name: &str) -> String {
    if is_standard_column(name) {
        format!("{alias}.\"{name}\"")
    } else {
        format!("({alias}.metadata ->> '{}')", lit(name))
    }
}

/// Is a prefix safe to build a column name from?
///
/// It becomes an identifier a comparison references by name, and it comes from
/// a tenant's config — so it is checked here rather than trusted. Refusing is
/// right: a prefix that cannot be a column name would otherwise produce SQL
/// that fails mid-run, long after the publish that accepted it.
#[must_use]
pub fn valid_prefix(prefix: &str) -> bool {
    !prefix.is_empty()
        && prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !prefix.starts_with(|c: char| c.is_ascii_digit())
}

/// Everything wrong with a `features.related:` block, as sentences.
///
/// Checked at publish rather than at run time, for the reason the rest of the
/// config is: a tenant who publishes a broken feature should be told so by the
/// call that publishes it, not by a job that fails an hour later with a SQL
/// error naming a column they never wrote.
#[must_use]
pub fn validate(config: &Config) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for feature in &config.features.related {
        if !valid_prefix(&feature.prefix) {
            problems.push(format!(
                "features.related: prefix '{}' is not a usable column-name prefix \
                 (letters, digits and underscores, not starting with a digit)",
                feature.prefix
            ));
        }
        if seen.contains(&feature.prefix.as_str()) {
            // Two features with one prefix write the same column twice, and the
            // second silently wins. Refusing names the collision instead.
            problems.push(format!(
                "features.related: prefix '{}' is declared twice; the second \
                 would overwrite the first's columns",
                feature.prefix
            ));
        }
        seen.push(&feature.prefix);
        if feature.attributes.is_empty() {
            problems.push(format!(
                "features.related['{}']: borrows no attributes, so it would join \
                 the corpus to itself and produce nothing",
                feature.prefix
            ));
        }
        if feature.key_column.trim().is_empty() {
            problems.push(format!(
                "features.related['{}']: key_column is required",
                feature.prefix
            ));
        }
        if !config.sources.contains_key(&feature.target_source) {
            let known: Vec<&str> = config.sources.keys().map(String::as_str).collect();
            problems.push(format!(
                "features.related['{}']: target_source '{}' is not in sources \
                 (known: {})",
                feature.prefix,
                feature.target_source,
                known.join(", ")
            ));
        }
        for attribute in &feature.attributes {
            if !valid_prefix(attribute) {
                problems.push(format!(
                    "features.related['{}']: attribute '{attribute}' is not a \
                     usable column name",
                    feature.prefix
                ));
            }
        }
    }
    problems
}

/// The SQL that widens `std_qual` with every declared feature's columns.
///
/// Returns `None` when nothing is declared — and the caller then registers the
/// standardized table itself, unchanged. That `None` is the determinism
/// property: there is no widened view to differ from today's input, rather than
/// a widened view that happens to be equal.
///
/// A LEFT join, always: a record whose key points at nothing is still a record,
/// and an inner join would silently drop it from the corpus — which would
/// change the partition, not just the comparison.
#[must_use]
pub fn widen_sql(config: &Config, std_qual: &str) -> Option<String> {
    let features = &config.features.related;
    if features.is_empty() {
        return None;
    }
    let mut selects = vec!["s.*".to_string()];
    let mut joins = Vec::new();
    for (index, feature) in features.iter().enumerate() {
        let alias = format!("r{index}");
        for attribute in &feature.attributes {
            selects.push(format!(
                "{} as \"{}\"",
                read_column(&alias, attribute),
                feature.column_for(attribute)
            ));
        }
        // The key is a BARE id while `record_key` is `source:id`, so the target
        // source is part of the join rather than guessed. Two sources sharing an
        // id space would otherwise cross-link silently.
        joins.push(format!(
            "left join {std_qual} {alias}\n       on {alias}.record_key = '{}' || ':' || {}",
            lit(&feature.target_source),
            read_column("s", &feature.key_column)
        ));
    }
    Some(format!(
        "select\n    {}\n  from {std_qual} s\n  {}",
        selects.join(",\n    "),
        joins.join("\n  ")
    ))
}

/// Every column the declared features contribute, in declaration order.
///
/// The config studio and the comparison validator both need this: a comparison
/// may name a borrowed column, and nothing else knows those columns exist.
#[must_use]
pub fn derived_columns(config: &Config) -> Vec<String> {
    config
        .features
        .related
        .iter()
        .flat_map(RelatedFeature::columns)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One `features.related` entry, as YAML. A builder rather than inline
    /// literals: the block is six lines of significant whitespace and every
    /// test needs a slightly different one.
    fn feature(prefix: &str, key: &str, target: &str, attributes: &str) -> String {
        [
            format!("    - prefix: {prefix}"),
            format!("      key_column: {key}"),
            format!("      target_source: {target}"),
            format!("      attributes: {attributes}"),
            String::new(),
        ]
        .join("\n")
    }

    fn config_with(features: &[String]) -> Config {
        let base = include_str!("../../er-config/tests/fixtures/default.yaml");
        let yaml = if features.is_empty() {
            base.to_string()
        } else {
            format!("{base}\nfeatures:\n  related:\n{}", features.concat())
        };
        er_config::load_config_str(&yaml).expect("fixture + features parses")
    }

    fn employer() -> String {
        feature("employer", "employer_id", "crm", "[given_name]")
    }

    #[test]
    fn no_block_means_no_widening() {
        // THE determinism property, and it is structural rather than asserted:
        // with nothing declared there is no widened view at all, so the engine
        // is handed exactly the table it is handed today and the four golden
        // suites that read through the concat stage cannot move.
        let config = config_with(&[]);
        assert!(config.features.is_empty());
        assert!(widen_sql(&config, "lake.main.int_std_records").is_none());
        assert_eq!(derived_columns(&config), Vec::<String>::new());
    }

    #[test]
    fn an_empty_block_serialises_away() {
        // The `config_hash` obligation: a config written before derived
        // features existed must hash byte-identically, or publishing it is
        // silently promoted to a re-score.
        let config = config_with(&[]);
        let doc = er_config::canonical_document(&config);
        assert!(
            !doc.contains("\"features\""),
            "an empty features block must not appear in the canonical document"
        );
    }

    #[test]
    fn a_declared_feature_moves_the_hash() {
        // The other half: a block that does NOT move the hash would be unhashed
        // config, which is worse than no block.
        let plain = config_with(&[]);
        let widened = config_with(&[employer()]);
        assert_ne!(
            er_config::config_hash(&plain),
            er_config::config_hash(&widened)
        );
    }

    #[test]
    fn borrows_a_standard_column_directly_and_metadata_by_key() {
        let config = config_with(&[feature(
            "employer",
            "employer_id",
            "crm",
            "[given_name, industry]",
        )]);
        let sql = widen_sql(&config, "lake.main.int_std_records").expect("a feature is declared");
        // A canonical attribute is read as a column...
        assert!(
            sql.contains("r0.\"given_name\" as \"employer_given_name\""),
            "{sql}"
        );
        // ...and anything else out of `metadata`, which is where every unmapped
        // source column already lands. A foreign key almost never IS canonical,
        // so this is the common path rather than the exception.
        assert!(
            sql.contains("(r0.metadata ->> 'industry') as \"employer_industry\""),
            "{sql}"
        );
        // Parenthesised, and asserted together with the concatenation it sits
        // inside: that is the shape DuckDB 2.0's `->>`/`||` reordering broke,
        // turning the key lookup into a JSON parse of `crm:{}`.
        assert!(
            sql.contains("'crm' || ':' || (s.metadata ->> 'employer_id')"),
            "{sql}"
        );
    }

    #[test]
    fn joins_on_the_target_source_not_the_bare_id() {
        // `record_key` is `source:id` and the key column holds a bare id. Two
        // sources sharing an id space would cross-link silently without this.
        let config = config_with(&[feature(
            "employer",
            "employer_id",
            "billing",
            "[given_name]",
        )]);
        let sql = widen_sql(&config, "lake.main.int_std_records").unwrap();
        assert!(sql.contains("'billing' || ':' ||"), "{sql}");
    }

    #[test]
    fn always_a_left_join() {
        // A record whose key points at nothing is still a record. An inner join
        // would drop it from the CORPUS, which changes the partition rather
        // than just the comparison — a far worse failure than a null column.
        let config = config_with(&[employer()]);
        let sql = widen_sql(&config, "lake.main.int_std_records").unwrap();
        assert!(sql.contains("left join"), "{sql}");
        assert!(!sql.contains("inner join"), "{sql}");
    }

    #[test]
    fn two_features_get_their_own_aliases() {
        let config = config_with(&[
            employer(),
            feature("household", "household_id", "billing", "[addr_city]"),
        ]);
        let sql = widen_sql(&config, "lake.main.int_std_records").unwrap();
        assert!(sql.contains("r0"), "{sql}");
        assert!(sql.contains("r1"), "{sql}");
        assert_eq!(
            derived_columns(&config),
            vec!["employer_given_name", "household_addr_city"]
        );
    }

    #[test]
    fn refuses_an_unknown_target_source() {
        // At PUBLISH, naming the alternatives. The run-time alternative is a
        // SQL error an hour later naming a column the tenant never wrote.
        let config = config_with(&[feature(
            "employer",
            "employer_id",
            "nowhere",
            "[given_name]",
        )]);
        let problems = validate(&config);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("nowhere"), "{problems:?}");
        assert!(problems[0].contains("known:"), "{problems:?}");
    }

    #[test]
    fn refuses_a_prefix_that_cannot_be_a_column_name() {
        // It comes from a tenant's config and becomes an identifier, so it is
        // checked rather than trusted.
        let config = config_with(&[feature(
            "\"emp loyer; drop table x\"",
            "employer_id",
            "crm",
            "[given_name]",
        )]);
        let problems = validate(&config);
        assert!(
            problems
                .iter()
                .any(|p| p.contains("not a usable column-name prefix")),
            "{problems:?}"
        );
    }

    #[test]
    fn refuses_a_duplicate_prefix() {
        // Two features with one prefix write the same column twice and the
        // second silently wins.
        let config = config_with(&[
            feature("employer", "a_id", "crm", "[given_name]"),
            feature("employer", "b_id", "crm", "[family_name]"),
        ]);
        let problems = validate(&config);
        assert!(
            problems.iter().any(|p| p.contains("declared twice")),
            "{problems:?}"
        );
    }

    #[test]
    fn refuses_a_feature_that_borrows_nothing() {
        let config = config_with(&[feature("employer", "employer_id", "crm", "[]")]);
        let problems = validate(&config);
        assert!(
            problems.iter().any(|p| p.contains("borrows no attributes")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_valid_block_has_nothing_to_report() {
        let config = config_with(&[feature(
            "employer",
            "employer_id",
            "crm",
            "[given_name, family_name]",
        )]);
        assert_eq!(validate(&config), Vec::<String>::new());
    }
}
