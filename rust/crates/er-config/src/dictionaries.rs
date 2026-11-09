//! Versioned, directed replacement dictionaries used by preprocessing rules.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Config, TransformRule, is_sql_identifier_fragment};

/// Named exact-string mappings, canonically ordered for configuration hashing.
pub type ReplacementDictionaries = BTreeMap<String, BTreeMap<String, String>>;

/// How a named mapping applies to a field. Both modes match case-sensitively.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DictionaryMode {
    /// Match the complete value, like the existing inline dictionary operation.
    #[default]
    Value,
    /// Match each space-separated word once; replacements are not reprocessed.
    Words,
}

impl DictionaryMode {
    #[must_use]
    pub fn is_value(&self) -> bool {
        *self == Self::Value
    }
}

/// Bound configuration size before expanding mappings into generated SQL.
pub const MAX_DICTIONARIES: usize = 128;
pub const MAX_ENTRIES: usize = 2_000;
pub const MAX_VALUE_BYTES: usize = 1_024;

/// Validate the registry and every reference before publication or execution.
///
/// # Errors
/// Invalid names, oversized/invalid mappings, missing references or word keys.
pub fn validate(config: &Config) -> Result<(), String> {
    if config.replacement_dictionaries.len() > MAX_DICTIONARIES {
        return Err(format!(
            "at most {MAX_DICTIONARIES} named dictionaries are allowed"
        ));
    }
    for (name, entries) in &config.replacement_dictionaries {
        if name.len() > 64 || !is_sql_identifier_fragment(name) {
            return Err(format!(
                "name {name:?} must be an identifier of at most 64 bytes"
            ));
        }
        if entries.len() > MAX_ENTRIES {
            return Err(format!("{name:?} has more than {MAX_ENTRIES} entries"));
        }
        for (from, to) in entries {
            if from.is_empty()
                || [from, to]
                    .into_iter()
                    .any(|s| s.len() > MAX_VALUE_BYTES || s.chars().any(char::is_control))
            {
                return Err(format!(
                    "{name:?} needs nonempty source values, no control characters and values of at most {MAX_VALUE_BYTES} bytes"
                ));
            }
        }
    }
    for rules in config.transforms.values() {
        for rule in rules {
            let TransformRule::NamedDictionary { dictionary, mode } = rule else {
                continue;
            };
            let entries = config
                .replacement_dictionaries
                .get(dictionary)
                .ok_or_else(|| {
                    format!("reference {dictionary:?} does not name a replacement dictionary")
                })?;
            if *mode == DictionaryMode::Words
                && entries
                    .keys()
                    .any(|key| key.chars().any(char::is_whitespace))
            {
                return Err(format!(
                    "{dictionary:?}: word matching requires source keys without whitespace"
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn render(
    entries: Option<&BTreeMap<String, String>>,
    mode: DictionaryMode,
    expr: &str,
) -> String {
    let Some(entries) = entries else {
        // Parsed configurations cannot reach this arm. Programmatically built
        // ones must fail visibly rather than silently skip a missing mapping.
        return "error('a named replacement dictionary is missing')".into();
    };
    if entries.is_empty() {
        return expr.into();
    }
    let rule = TransformRule::Dictionary {
        map: entries.clone(),
    };
    match mode {
        DictionaryMode::Value => rule.apply_sql(expr),
        DictionaryMode::Words => format!(
            "array_to_string(list_transform(string_split({expr}, ' '), lambda word: {}), ' ')",
            rule.apply_sql("word")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config_hash, load_config_str};

    const BASE: &str = include_str!("../tests/fixtures/test.yaml");

    #[test]
    fn empty_registry_preserves_hash_and_named_contents_are_versioned() {
        let base = load_config_str(BASE).unwrap();
        let empty = load_config_str(&format!("{BASE}\nreplacement_dictionaries: {{}}\n")).unwrap();
        assert_eq!(config_hash(&base), config_hash(&empty));
        let yaml = format!(
            "{BASE}\nreplacement_dictionaries:\n  street_types:\n    Strasse: Street\ntransforms:\n  address_line:\n    - op: named_dictionary\n      dictionary: street_types\n      mode: words\n"
        );
        let named = load_config_str(&yaml).unwrap();
        assert_ne!(config_hash(&base), config_hash(&named));
        let roundtrip = load_config_str(&serde_norway::to_string(&named).unwrap()).unwrap();
        assert_eq!(named, roundtrip);
        let mut changed = named.clone();
        changed
            .replacement_dictionaries
            .get_mut("street_types")
            .unwrap()
            .insert("Strasse".into(), "Avenue".into());
        assert_ne!(config_hash(&named), config_hash(&changed));
    }

    #[test]
    fn missing_and_invalid_named_dictionaries_are_refused_at_load() {
        for extra in [
            "transforms:\n  address_line: [{op: named_dictionary, dictionary: absent}]",
            "replacement_dictionaries: {'bad-name': {Street: St}}",
            "replacement_dictionaries: {street_types: {'': St}}",
            "replacement_dictionaries: {street_types: {Street: \"St\\n\"}}",
            "replacement_dictionaries: {street_types: {'Main Street': Main}}\ntransforms:\n  address_line: [{op: named_dictionary, dictionary: street_types, mode: words}]",
        ] {
            assert!(
                load_config_str(&format!("{BASE}\n{extra}\n")).is_err(),
                "{extra}"
            );
        }
        let mut oversized = load_config_str(BASE).unwrap();
        oversized.replacement_dictionaries.insert(
            "large".into(),
            (0..=MAX_ENTRIES)
                .map(|i| (i.to_string(), "x".into()))
                .collect(),
        );
        assert!(validate(&oversized).is_err());
    }
}
