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

/// The top-level blocks of S6, in document order — the count is the array's own
/// length, deliberately not restated in prose (it has read "fourteen" since it
/// held sixteen). [`Config`]'s S6 block fields MUST equal this set; the `entity_type` discriminator (added for the
/// Account entity type) is a meta-field, not an S6 block, and is excluded.
pub const CONFIG_BLOCKS: [&str; 19] = [
    "tenant",
    "thresholds",
    "models",
    "standardization",
    "transforms",
    "sources",
    "blocking",
    "constraints",
    "comparisons",
    "segments",
    "data_quality",
    "survivorship",
    "training",
    "storage",
    "versions",
    "generator",
    "clustering",
    "coherence",
    "correction_pass",
];

/// S6 `transforms`: ordered cleanse rules per canonical attribute, applied to
/// the raw value BEFORE standardization.
///
/// Before standardization, not after, and that ordering is the point: the
/// normalizers decide what an email or a phone number IS, and feeding them
/// `"  BOB@EXAMPLE.COM "` or `"(415) 555-0100"` wastes their judgement on
/// noise a cleanse rule removes for free. A rule that ran afterwards would be
/// editing the engine's conclusion instead of its input.
///
/// Every rule is a pure scalar expression over one column, so the whole chain
/// compiles into the staging projection and costs nothing extra to run. Ops
/// that need two columns (concat) are deliberately absent for now rather than
/// half-present: they change the projection's shape, not just a value.
///
/// Absent from the canonical document when empty, so `config_hash` — and every
/// `std_hashes` golden — is unchanged for a config with no rules. A config that
/// DOES declare rules changes the standardized corpus, which moves
/// `config_hash` and so promotes the next incremental run to a full one through
/// the existing drift guard. That is correct: the corpus was standardized
/// differently.
pub type Transforms = BTreeMap<String, Vec<TransformRule>>;

/// One cleanse operation. `op` is the discriminator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum TransformRule {
    /// Lowercase.
    Lower,
    /// Uppercase.
    Upper,
    /// Strip leading and trailing whitespace.
    Trim,
    /// Literal substring replacement.
    Replace { from: String, to: String },
    /// Regular-expression replacement, every occurrence.
    RegexReplace {
        pattern: String,
        #[serde(default)]
        replacement: String,
    },
    /// Remove every one of these characters. A chain of literal replacements
    /// rather than a character class, so there is no escaping to get wrong and
    /// `-` or `]` behave like any other character.
    Strip { chars: String },
    /// 1-based substring.
    Substring { start: i64, length: i64 },
    /// Replace an empty or missing value with a constant. Trims first, so a
    /// whitespace-only cell counts as missing.
    Default { value: String },
    /// Map exact values to replacements; anything unmatched passes through.
    Dictionary { map: BTreeMap<String, String> },
}

impl TransformRule {
    /// Wrap `expr` in this rule, returning a scalar SQL expression.
    ///
    /// String literals are escaped by doubling quotes — these values come from a
    /// tenant's config, which is authored by an admin but is still input.
    #[must_use]
    pub fn apply_sql(&self, expr: &str) -> String {
        let lit = |v: &str| format!("'{}'", v.replace('\'', "''"));
        match self {
            Self::Lower => format!("lower({expr})"),
            Self::Upper => format!("upper({expr})"),
            Self::Trim => format!("trim({expr})"),
            Self::Replace { from, to } => {
                format!("replace({expr}, {}, {})", lit(from), lit(to))
            }
            Self::RegexReplace {
                pattern,
                replacement,
            } => format!(
                "regexp_replace({expr}, {}, {}, 'g')",
                lit(pattern),
                lit(replacement)
            ),
            Self::Strip { chars } => chars.chars().fold(expr.to_string(), |acc, c| {
                format!("replace({acc}, {}, '')", lit(&c.to_string()))
            }),
            Self::Substring { start, length } => {
                format!("substr({expr}, {start}, {length})")
            }
            Self::Default { value } => {
                format!("coalesce(nullif(trim({expr}), ''), {})", lit(value))
            }
            Self::Dictionary { map } => {
                let mut arms = String::new();
                for (from, to) in map {
                    use std::fmt::Write as _;
                    let _ = write!(arms, " when {} then {}", lit(from), lit(to));
                }
                // `else` passes the value through: a dictionary is a correction
                // list, not an allowlist, and nulling the unmatched values would
                // silently delete most of a column.
                format!("case {expr}{arms} else {expr} end")
            }
        }
    }
}

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
/// paths land in later sub-PRs). A meta-discriminator, NOT one of the S6
/// [`CONFIG_BLOCKS`].
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

/// One member of the matching ensemble (E5b): a registry model name, the
/// threshold its own edges auto-merge at, and how it combines with the members
/// before it.
///
/// The name refers to a `matching_models` row's `name`, whose ACTIVE version is
/// what a run scores with — so retraining a member is an activate in the
/// registry, not a config publish. That is also why activation had to become
/// unique per `(org, name)` rather than per org (E5b-1): several members are
/// active at once by definition.
///
/// Members UNION at the corpus threshold: each scores the whole candidate set,
/// and a pair merges if any member put it at or above `thresholds.auto_merge`.
/// That is what the existing model-blind edge read already computes, which is
/// why this slice needs no new SQL and no change to reconcile.
///
/// Deliberately absent, each landing with the code that reads it rather than
/// before it — a declared field nothing honours is the dead-config shape this
/// program has already found once:
///
/// * a per-member `threshold`. It needs the resolved `model_version ->
///   threshold` mapping available wherever clustering happens, and that mapping
///   lives in the registry (Postgres) while `run_correct` re-clusters from the
///   lake alone. Persisting the resolved ensemble on the run row is the fix, and
///   it is DDL.
/// * a `mode` (`and_run` / `then_run`). The cascade is a predicate on which
///   pairs a later member scores at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSpec {
    /// The registry name whose active version this member scores with.
    pub name: String,
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

/// What a missing value means for one comparison.
///
/// The default is deliberately NOT the competitor's. DataGroomr defaults to
/// "match if both blank", which its own users hit as a false-positive trap: a
/// thousand records with no phone number are not a thousand copies of one
/// person. Here a blank is *no evidence* — the pair is scored on its other
/// fields — and a tenant that wants the other behaviour asks for it by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NullPolicy {
    /// A missing value on either side contributes nothing (the engine's
    /// `NullLevel`: gamma -1, excluded from the score).
    #[default]
    Never,
    /// Two blanks are treated as agreement and carry weight.
    BothBlankMatches,
    /// A blank on either side is treated as agreement. Rarely what anyone
    /// wants; offered because migrating an existing rule set sometimes needs it.
    EitherBlankMatches,
}

/// S6 `constraints`: hard rules about which pairs may be considered at all,
/// applied BEFORE any similarity is scored.
///
/// These are not weights. A weight says "this field matters more"; a constraint
/// says "without this, do not even look". That distinction is the whole point:
/// a probabilistic score can always be outvoted by enough weak agreement, which
/// is how a matcher merges two different people who happen to share a city and
/// a first name. A constraint cannot be outvoted.
///
/// All three compile to conjuncts on every blocking rule, so a pair that
/// violates one is never generated as a candidate — the cheapest possible place
/// to enforce it, and the only place where "before similarity counts" is
/// literally true.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchConstraints {
    /// Columns that must be EQUAL and NON-BLANK for a pair to be a candidate.
    /// The matrix's "required match" / "must-match key": a tax id or account
    /// number that has to agree before similarity means anything.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_match: Vec<String>,
    /// Columns that VETO a pair when both sides are present and they differ.
    /// Different from `required_match`: a missing value is tolerated (plenty of
    /// records have no SSN), but two different non-null values are proof these
    /// are different people. This is what prevents the catastrophic false merge
    /// on a strong identifier.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub veto: Vec<String>,
    /// Columns whose EQUALITY excludes a pair — the relationship exclusion.
    /// Records sharing a household, a parent account or a joint-account id are
    /// related, not duplicates, and they are the classic false positive in
    /// person resolution: two people at one address with one surname.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_when_equal: Vec<String>,
}

impl MatchConstraints {
    /// Whether anything is declared. An empty block compiles to no conjuncts at
    /// all, which is what keeps an existing config's model byte-identical.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.required_match.is_empty() && self.veto.is_empty() && self.exclude_when_equal.is_empty()
    }

    /// Every column the block names, for validating them against the schema.
    pub fn columns(&self) -> impl Iterator<Item = &str> {
        self.required_match
            .iter()
            .chain(&self.veto)
            .chain(&self.exclude_when_equal)
            .map(String::as_str)
    }
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
    /// This field's share of the score, 0..=100. When any comparison declares a
    /// weight they all must, and they must total 100 — a silently-defaulted
    /// weight would drop a field out of the model. When none declares one, the
    /// comparisons share the score equally.
    ///
    /// `skip_serializing_if` is load-bearing: the canonical document feeds
    /// `config_hash`, so an absent weight must not serialize, or every existing
    /// tenant config would change hash and trigger a rebuild.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
    /// What a blank means here. Omitted from the canonical document when it is
    /// the default, for the same hash-stability reason as `weight`.
    #[serde(default, skip_serializing_if = "is_default_null_policy")]
    pub null_policy: NullPolicy,
}

/// `skip_serializing_if` predicate for [`ComparisonSpec::null_policy`].
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_default_null_policy(policy: &NullPolicy) -> bool {
    *policy == NullPolicy::Never
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

/// S6 `segments`: named slices of the corpus, `name -> SQL predicate` over the
/// standardized columns (e.g. `west: "addr_region in ('CA','OR','WA')"`).
///
/// A segment bounds how much work ONE run does, not what it is allowed to see.
/// The scoped pass scores the segment against the **whole** corpus — segment ×
/// everything, plus segment × segment — and reuses the stored scores for pairs
/// where neither endpoint is in the segment. With the model and the TF snapshot
/// frozen those stored numbers are exactly what a re-score would produce, so a
/// scoped run leaves `match_scores` in the state a full run would, and running
/// each segment in turn covers the corpus with no pair left unscored. That is
/// the one thing an operator actually needs from "split your dataset", and the
/// obvious reading — filter the corpus down to the segment and resolve that —
/// cannot give it, because every cross-segment duplicate would be invisible and
/// records outside the slice would lose the entities they are already in.
///
/// The predicate is declared in config and referenced **by name** at run time,
/// never supplied with the request: a job carrying raw SQL would be an injection
/// surface, and a name can be checked against the published config. Every
/// predicate is parsed and column-checked at publish, like a blocking rule.
pub type Segments = BTreeMap<String, String>;

/// S6 `data_quality`: how a per-record quality score is computed.
///
/// Three dimensions, because they fail independently and a single number that
/// mixes them tells an operator nothing about what to fix: a record can be
/// COMPLETE and wrong (a malformed email in every field), VALID and empty (one
/// good email and nothing else), or both and five years out of date.
///
/// Each dimension is scored only where it APPLIES. A record with no email is
/// already penalised for that by completeness; scoring its validity as zero as
/// well would penalise the same absence twice and make "fill in the blanks"
/// look like it fixes validity. So validity is measured over the validatable
/// fields that are PRESENT, and a record with none of them is scored on the
/// other dimensions alone rather than being given a zero it did not earn.
///
/// Absent from the canonical document when unset, so `config_hash` is unchanged
/// for every config written before quality scoring existed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataQuality {
    /// Attributes that count toward completeness, with their weights. A field
    /// nobody uses should not drag a record's score down, so this is declared
    /// rather than "every column".
    pub completeness: BTreeMap<String, f64>,
    /// Relative weight of each dimension in the final score.
    #[serde(default = "default_dq_weights")]
    pub weights: DataQualityWeights,
    /// A record whose source timestamp is this recent scores full marks for
    /// freshness.
    #[serde(default = "default_fresh_days")]
    pub fresh_days: i64,
    /// A record this old scores zero for freshness; in between it decays
    /// linearly. Must exceed `fresh_days`.
    #[serde(default = "default_stale_days")]
    pub stale_days: i64,
}

/// How much each dimension contributes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataQualityWeights {
    pub completeness: f64,
    pub validity: f64,
    pub freshness: f64,
}

fn default_dq_weights() -> DataQualityWeights {
    DataQualityWeights {
        completeness: 1.0,
        validity: 1.0,
        freshness: 1.0,
    }
}

const fn default_fresh_days() -> i64 {
    90
}

const fn default_stale_days() -> i64 {
    1095
}

impl DataQuality {
    /// Whether scoring is configured at all. Used by `skip_serializing_if` so an
    /// unset block leaves `config_hash` untouched.
    #[must_use]
    pub fn is_unset(&self) -> bool {
        self.completeness.is_empty()
    }
}

impl Default for DataQuality {
    fn default() -> Self {
        Self {
            completeness: BTreeMap::new(),
            weights: default_dq_weights(),
            fresh_days: default_fresh_days(),
            stale_days: default_stale_days(),
        }
    }
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
    /// The matching ensemble: an ordered list of registry model names, each
    /// with its own threshold and combine mode (E5b). Empty means the
    /// single-model path — the org's one active model, scored at
    /// `thresholds.auto_merge` — which is what every config written before
    /// ensembles existed means, and why this is omitted from the canonical
    /// document when empty: `config_hash` stays byte-identical for them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelSpec>,
    pub standardization: Standardization,
    /// Cleanse rules per canonical attribute, applied before standardization.
    /// Omitted from the canonical document when empty, so a config written
    /// before transforms existed hashes identically.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub transforms: Transforms,
    pub sources: BTreeMap<String, SourceSpec>,
    pub blocking: Vec<BlockingRule>,
    /// Hard candidate rules. Omitted from the canonical document when empty, so
    /// every config written before constraints existed hashes identically.
    #[serde(default, skip_serializing_if = "MatchConstraints::is_empty")]
    pub constraints: MatchConstraints,
    pub comparisons: BTreeMap<String, ComparisonSpec>,
    /// Named slices of the corpus a run may be restricted to. Omitted from the
    /// canonical document when empty, so a config written before segments
    /// existed hashes identically.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub segments: BTreeMap<String, String>,
    /// How a per-record quality score is computed. Omitted from the canonical
    /// document when unset, so a config written before quality scoring existed
    /// hashes identically.
    #[serde(default, skip_serializing_if = "DataQuality::is_unset")]
    pub data_quality: DataQuality,
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
    // The serde error is the `#[from]` source; it is not interpolated here, so
    // a reporter walking `.source()` prints the line/column detail once.
    #[error("config is not valid YAML or does not match S6")]
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
        assert_eq!(CONFIG_BLOCKS.len(), 19);
        assert_eq!(CONFIG_BLOCKS[0], "tenant");
        assert_eq!(CONFIG_BLOCKS[18], "correction_pass");
        // `models` sits beside `thresholds` because it refines the same
        // decision — what probability merges — one threshold per member
        // instead of one for the corpus.
        assert!(CONFIG_BLOCKS.contains(&"models"));
        // `constraints` sits beside `blocking` because it changes the same
        // thing — which pairs are candidates — and so carries the same publish
        // tier.
        assert!(CONFIG_BLOCKS.contains(&"constraints"));
        // `segments` sits after `comparisons` because it changes neither what a
        // pair scores nor which pairs are candidates — only how much of the
        // corpus one run re-scores.
        assert!(CONFIG_BLOCKS.contains(&"segments"));
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
    fn an_empty_models_block_serialises_away() {
        // The obligation `skip_serializing_if` exists for: every config written
        // before ensembles must hash byte-identically, so publishing one is not
        // silently promoted to a tier-B re-score and no `std_hashes` golden
        // moves. Asserted on the canonical document (what `config_hash` is taken
        // over), not just on the field.
        let config = load_config_str(DEFAULT_YAML).unwrap();
        assert!(config.models.is_empty(), "the fixture declares no ensemble");
        let doc = canonical_document(&config);
        assert!(
            !doc.contains("\"models\""),
            "an empty models block must not appear in the canonical document"
        );
        // And a declared ensemble DOES move the hash — otherwise the block
        // would be unhashed config, which is worse than no block at all.
        let mut with_models = config.clone();
        with_models.models.push(ModelSpec {
            name: "strict".to_string(),
        });
        assert_ne!(
            config_hash(&config),
            config_hash(&with_models),
            "declaring an ensemble must change config_hash"
        );
        // A member that takes every default serialises to just its name, so the
        // document stays as small as what it actually says.
        let doc = canonical_document(&with_models);
        assert!(
            doc.contains(r#""models":[{"name":"strict"}]"#),
            "defaults must not be written out, got {doc}"
        );
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
