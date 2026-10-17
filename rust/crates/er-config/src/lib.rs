//! The S6 configuration document and its `config_hash` (port of
//! `src/er/config/{schema,hashing,loader}.py`, DesignDoc.md S6/S6.1/S5.2).
//!
//! `config_hash` (S5.2) is the SHA-256 of the validated, S6.1-normalized document
//! serialised as JSON with **sorted keys, compact `,`/`:` separators, UTF-8, no
//! ASCII escaping**. The ordering is why the port is byte-identical to Python
//! without reproducing field order: Python uses `json.dumps(sort_keys=True)`, and
//! here [`canonical_document`] routes through `serde_json::Value` whose object is a
//! `BTreeMap` (keys sorted) and whose `to_string` is already compact. The digest is
//! taken over the post-normalisation document, so two configs differing only by a
//! redundant `null` level or a missing terminal survivorship rule hash identically.
//!
//! Every block is `deny_unknown_fields` — the S6 `extra='forbid'` rule: a key the
//! spec does not name is a typo, and a typo that validates is a tunable silently
//! taking its default. The cross-field validators V1–V16 are not yet ported; this
//! crate parses, normalises, and hashes (the S5.2 contract).

use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// S6.1 appends this as the terminal element of every survivorship chain, making
/// each chain a total order over an entity's records.
pub const TERMINAL_SURVIVORSHIP_RULE: &str = "record_key ASC";

/// Source names beginning with this prefix are reserved for engine-synthesized
/// sources and rejected in a user document (S6.1 V11).
pub const RESERVED_SOURCE_PREFIX: &str = "_";

/// The fourteen top-level blocks of S6, in document order. [`Config`]'s S6 block
/// fields MUST equal this set; the `entity_type` discriminator (added for the
/// Account entity type) is a meta-field, not an S6 block, and is excluded.
pub const CONFIG_BLOCKS: [&str; 14] = [
    "tenant",
    "thresholds",
    "standardization",
    "sources",
    "blocking",
    "comparisons",
    "survivorship",
    "training",
    "storage",
    "versions",
    "generator",
    "clustering",
    "coherence",
    "correction_pass",
];

/// The canonical attributes the S4.2 standardization macros consume; every
/// `sources.<name>.columns` mapping must cover all of them (V11).
pub const CANONICAL_ATTRIBUTES: [&str; 9] = [
    "given_name",
    "family_name",
    "email",
    "phone",
    "address_line",
    "addr_city",
    "addr_region",
    "addr_postal",
    "birth_date",
];

/// The entity type a tenant's config resolves. `Person` is the default and the
/// only type the existing pipeline standardizes/matches today; `Account` selects
/// the parallel Account attribute set + schema (the standardize/match/frontend
/// paths land in later sub-PRs). A meta-discriminator, NOT one of the fourteen
/// S6 [`CONFIG_BLOCKS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    #[default]
    Person,
    Account,
}

impl EntityType {
    /// Whether this is the default `Person` type. Used by [`Config`]'s
    /// `skip_serializing_if` so a person config serialises byte-identically to a
    /// pre-`entity_type` document — `config_hash` is unchanged for every existing
    /// (person) tenant.
    #[must_use]
    pub fn is_person(&self) -> bool {
        matches!(self, EntityType::Person)
    }
}

/// The canonical attributes for the Account/Org entity type (the `Account`
/// analogue of [`CANONICAL_ATTRIBUTES`]); every `sources.<name>.columns` mapping
/// for an Account config must cover all of them.
pub const ACCOUNT_CANONICAL_ATTRIBUTES: [&str; 7] = [
    "company_name",
    "domain",
    "phone",
    "addr_street",
    "addr_city",
    "addr_region",
    "addr_postal",
];

/// The canonical attribute set for an entity type: the person nine or the
/// Account seven. The source-mapping coverage contract (V11) is enforced against
/// whichever set this returns.
#[must_use]
pub fn canonical_attributes(entity_type: EntityType) -> &'static [&'static str] {
    match entity_type {
        EntityType::Person => &CANONICAL_ATTRIBUTES,
        EntityType::Account => &ACCOUNT_CANONICAL_ATTRIBUTES,
    }
}

/// S6 `thresholds` (units + gray band are normative in S4.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Thresholds {
    pub auto_merge: f64,
    pub review_low: f64,
}

/// S6 `standardization`, consumed by `email_norm` / `phone_e164`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Standardization {
    pub email_strip_plus_addressing: bool,
    pub email_placeholders: Vec<String>,
    pub phone_default_region: String,
}

/// S6 `sources.<name>`: one delivering system and its column mapping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpec {
    pub adapter: String,
    pub priority_rank: i64,
    pub record_id_column: String,
    pub updated_at_column: String,
    pub date_format: String,
    /// canonical attribute -> the column that carries it in this source, in
    /// YAML-declared order. The order is load-bearing: ingest's `content_hash`
    /// (S4.1) hashes `columns.values()` in exactly this order, so it is an
    /// `IndexMap`, not a sorted map. (`config_hash` re-sorts it, so the digest is
    /// order-independent — only the ingest preimage is not.)
    pub columns: IndexMap<String, String>,
}

/// S6 `blocking[]`: one candidate-generation key and the expression that builds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockingRule {
    pub key_type: String,
    pub expr: String,
}

/// S6 `comparisons.<column>`: the comparison levels and whether TF adjustment applies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonSpec {
    /// A YAML `null` token is the explicit `NullLevel`; S6.1 normalization removes
    /// it (the comparison builder always emits `NullLevel` first regardless).
    pub levels: Vec<Option<String>>,
    #[serde(default)]
    pub tf: bool,
}

/// S6 `training.em`: the arguments of every EM session (S4.3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrainingEm {
    pub fix_u_probabilities: bool,
    #[serde(default)]
    pub max_pairs: Option<i64>,
}

/// S6 `training`. Every argument of the S4.3.2 call sequence comes from here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Training {
    pub deterministic_rules: Vec<String>,
    pub recall: f64,
    pub u_max_pairs: i64,
    /// REQUIRED with no default (V10): a defaulted seed makes the training
    /// determinism test's byte-equality claim unreproducible.
    pub u_seed: i64,
    #[serde(default = "default_sampling_method")]
    pub u_sampling_method: String,
    #[serde(default)]
    pub u_min_count_per_level: Option<i64>,
    #[serde(default = "default_num_chunks")]
    pub u_num_chunks: i64,
    pub em_blocking_rules: Vec<String>,
    pub em: TrainingEm,
}

fn default_sampling_method() -> String {
    "bernoulli".to_string()
}

fn default_num_chunks() -> i64 {
    10
}

/// S6 `storage`: where the lake, the drop folder and the model artifacts live.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Storage {
    pub data_path: String,
    pub drop_dir: String,
    pub model_uri_prefix: String,
}

/// S6 `versions`: the version strings the stages stamp into their output rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Versions {
    pub std_version: String,
    pub survivorship_version: String,
    pub address_parser_version: String,
}

/// S6 `generator`: the seed of the synthetic-data generator (S8, S10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generator {
    pub seed: i64,
}

/// S6 `clustering`: the label-propagation bounds (S4.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clustering {
    pub max_iterations: i64,
    pub cut_protect_probability: f64,
}

/// S6 `coherence`: the phase-2 embedding scorer; `noop` in v1 (S11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coherence {
    pub scorer: String,
}

/// S6 `correction_pass`: when the periodic full-corpus correction runs (S4.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectionPass {
    pub cadence: String,
}

/// The whole S6 document, validated. `config_hash` is NOT a field of it (S6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The entity type this config resolves. Defaults to `Person` and is skipped
    /// in serialisation when `Person`, so every existing (person) config hashes
    /// byte-identically — `config_hash` is unchanged. Not an S6 block.
    #[serde(default, skip_serializing_if = "EntityType::is_person")]
    pub entity_type: EntityType,
    pub tenant: String,
    pub thresholds: Thresholds,
    pub standardization: Standardization,
    pub sources: BTreeMap<String, SourceSpec>,
    pub blocking: Vec<BlockingRule>,
    pub comparisons: BTreeMap<String, ComparisonSpec>,
    pub survivorship: BTreeMap<String, Vec<String>>,
    pub training: Training,
    pub storage: Storage,
    pub versions: Versions,
    pub generator: Generator,
    pub clustering: Clustering,
    pub coherence: Coherence,
    pub correction_pass: CorrectionPass,
}

/// A document that could not be read, parsed, or validated against S6. `code` is
/// the S4.0 exit status (2), matching Python's `ConfigValidationError`.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config is not valid YAML or does not match S6: {0}")]
    Parse(#[from] serde_norway::Error),
}

impl ConfigError {
    #[must_use]
    pub fn code(&self) -> i32 {
        2
    }
}

/// Apply the S6.1 post-validation normalization and return the new document:
/// drop the `null` token from every `comparisons[*].levels` list, and make
/// `record_key ASC` the terminal element of every survivorship chain. Both are
/// part of the canonical document `config_hash` covers.
#[must_use]
pub fn normalize(mut config: Config) -> Config {
    for comparison in config.comparisons.values_mut() {
        comparison.levels.retain(Option::is_some);
    }
    for chain in config.survivorship.values_mut() {
        if chain.last().map(String::as_str) != Some(TERMINAL_SURVIVORSHIP_RULE) {
            chain.push(TERMINAL_SURVIVORSHIP_RULE.to_string());
        }
    }
    config
}

/// Parse an S6 document from YAML text and apply S6.1 normalization — the form
/// `config_hash` is computed over. (Full V1–V16 validation is not yet ported.)
pub fn load_config_str(yaml: &str) -> Result<Config, ConfigError> {
    let config: Config = serde_norway::from_str(yaml)?;
    Ok(normalize(config))
}

/// The canonical JSON serialisation S5.2 hashes: sorted keys, compact separators,
/// UTF-8, no ASCII escaping. Routing through `serde_json::Value` (a `BTreeMap`
/// object) is what sorts the keys, matching Python's `json.dumps(sort_keys=True,
/// separators=(",", ":"), ensure_ascii=False)` over `model_dump(mode="json")`.
///
/// The caller passes the NORMALIZED document (what [`load_config_str`] returns).
#[must_use]
pub fn canonical_document(config: &Config) -> String {
    let value = serde_json::to_value(config).expect("Config serialises to JSON");
    serde_json::to_string(&value).expect("JSON value serialises")
}

/// SHA-256 hex digest of [`canonical_document`] (S5.2).
#[must_use]
pub fn config_hash(config: &Config) -> String {
    let mut hasher = Sha256::new();
    hasher.update(canonical_document(config).as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_YAML: &str = include_str!("../tests/fixtures/default.yaml");

    #[test]
    fn blocks_and_constants() {
        assert_eq!(CONFIG_BLOCKS.len(), 14);
        assert_eq!(CONFIG_BLOCKS[0], "tenant");
        assert_eq!(CONFIG_BLOCKS[13], "correction_pass");
        assert_eq!(CANONICAL_ATTRIBUTES.len(), 9);
        assert_eq!(TERMINAL_SURVIVORSHIP_RULE, "record_key ASC");
    }

    #[test]
    fn account_attribute_set() {
        assert_eq!(ACCOUNT_CANONICAL_ATTRIBUTES.len(), 7);
        assert_eq!(
            canonical_attributes(EntityType::Person),
            &CANONICAL_ATTRIBUTES
        );
        assert_eq!(
            canonical_attributes(EntityType::Account),
            &ACCOUNT_CANONICAL_ATTRIBUTES
        );
        assert!(EntityType::default().is_person());
        assert!(!EntityType::Account.is_person());
    }

    #[test]
    fn person_entity_type_serialises_away() {
        // The default (Person) entity_type must not appear in the canonical
        // document, so every existing person config hashes byte-identically.
        let config = load_config_str(DEFAULT_YAML).unwrap();
        assert!(config.entity_type.is_person());
        assert!(!canonical_document(&config).contains("entity_type"));
    }

    #[test]
    fn normalize_appends_terminal_and_drops_nulls() {
        let mut config = load_config_str(DEFAULT_YAML).unwrap();
        // every survivorship chain ends with the terminal rule exactly once
        for chain in config.survivorship.values() {
            assert_eq!(chain.last().unwrap(), TERMINAL_SURVIVORSHIP_RULE);
            assert_eq!(
                chain
                    .iter()
                    .filter(|r| *r == TERMINAL_SURVIVORSHIP_RULE)
                    .count(),
                1
            );
        }
        // normalization is idempotent
        let once = config_hash(&config);
        config = normalize(config);
        assert_eq!(once, config_hash(&config));
        // no null levels survive
        for comparison in config.comparisons.values() {
            assert!(comparison.levels.iter().all(Option::is_some));
        }
    }

    #[test]
    fn source_columns_preserve_declared_order() {
        // ingest's content_hash hashes columns.values() in YAML order, which is
        // the canonical-attribute order, NOT sorted — a sorted map would reorder
        // (addr_city would come first) and break the ingest preimage.
        let config = load_config_str(DEFAULT_YAML).unwrap();
        let crm = &config.sources["crm"];
        let order: Vec<&str> = crm.columns.keys().map(String::as_str).collect();
        assert_eq!(
            order,
            vec![
                "given_name",
                "family_name",
                "email",
                "phone",
                "address_line",
                "addr_city",
                "addr_region",
                "addr_postal",
                "birth_date",
            ]
        );
    }

    #[test]
    fn deny_unknown_fields() {
        let err = load_config_str("tenant: x\nnot_a_block: 1\n").unwrap_err();
        assert_eq!(err.code(), 2);
    }
}
