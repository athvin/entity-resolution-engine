//! The S5 relation registry — framework types + closed vocabularies (port of
//! `src/er/lake/model.py`). The `TableSpec` registry (`_DDL_SPECS`/`_DBT_SPECS`,
//! `create_table_sql`) lands in a later wake with a Python-generated golden-DDL
//! oracle; this is the dependency-free framework + every `in {…}` domain.

use std::collections::{BTreeMap, BTreeSet};

use crate::columns::{GOLDEN_LINEAGE_ATTRIBUTES, GOLDEN_SURVIVABLE_COLUMNS};

/// `lake.main` — the schema every relation materialises into (S5).
pub const SCHEMA_QUALIFIER: &str = "lake.main";

// The eight types S5 permits (fixed-size ARRAY is deliberately absent; DuckLake
// does not support it).
pub const VARCHAR: &str = "VARCHAR";
pub const BIGINT: &str = "BIGINT";
pub const DOUBLE: &str = "DOUBLE";
pub const BOOLEAN: &str = "BOOLEAN";
pub const DATE: &str = "DATE";
pub const TIMESTAMP: &str = "TIMESTAMP";
pub const JSON: &str = "JSON";
pub const LIST_VARCHAR: &str = "LIST(VARCHAR)";

pub const COLUMN_TYPES: [&str; 8] = [
    VARCHAR,
    BIGINT,
    DOUBLE,
    BOOLEAN,
    DATE,
    TIMESTAMP,
    JSON,
    LIST_VARCHAR,
];

#[must_use]
pub fn is_valid_type(type_: &str) -> bool {
    COLUMN_TYPES.contains(&type_)
}

/// The two — and only two — owners of a relation (S5.0, D14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Ddl,
    Dbt,
}

impl Owner {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Owner::Ddl => "ddl.py",
            Owner::Dbt => "dbt",
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SchemaError {
    #[error("{column}: {type_:?} is not one of the S5 types")]
    BadType { column: String, type_: String },
    #[error("{relation}: duplicate column")]
    DuplicateColumn { relation: String },
    #[error("{relation}: key column {column:?} is not a column")]
    KeyColumnMissing { relation: String, column: String },
    #[error("{relation}: enum column {column:?} is not a column")]
    EnumColumnMissing { relation: String, column: String },
    #[error("{relation}: dbt-owned; ddl.py must never issue DDL against it")]
    DbtOwnedDdl { relation: String },
}

/// One column of a relation, in S5 declaration order. `nullable` is the whole
/// constraint model — `NOT NULL` is the only constraint DuckLake enforces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub r#type: String,
    pub nullable: bool,
}

impl Column {
    /// A column, rejecting a type outside the eight S5 types.
    pub fn new(
        name: impl Into<String>,
        type_: impl Into<String>,
        nullable: bool,
    ) -> Result<Self, SchemaError> {
        let name = name.into();
        let type_ = type_.into();
        if !is_valid_type(&type_) {
            return Err(SchemaError::BadType {
                column: name,
                type_,
            });
        }
        Ok(Self {
            name,
            r#type: type_,
            nullable,
        })
    }

    /// A `NOT NULL` column — the only constraint a generated statement may carry.
    pub fn not_null(
        name: impl Into<String>,
        type_: impl Into<String>,
    ) -> Result<Self, SchemaError> {
        Self::new(name, type_, false)
    }

    /// The column's DDL fragment, `NOT NULL` included when it applies.
    #[must_use]
    pub fn sql(&self) -> String {
        format!(
            "{} {}{}",
            self.name,
            self.r#type,
            if self.nullable { "" } else { " NOT NULL" }
        )
    }
}

/// A key S5.0 declares, enforced by a dbt test and `MERGE INTO` — never the engine.
/// `columns` is empty only for `model_registry`'s at-most-one-active-row claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalKey {
    pub columns: Vec<String>,
    pub r#where: Option<String>,
}

/// One relation: owner, ordered columns, logical keys, `in {…}` domains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSpec {
    pub name: String,
    pub owner: Owner,
    pub columns: Vec<Column>,
    pub keys: Vec<LogicalKey>,
    pub enums: BTreeMap<String, BTreeSet<String>>,
}

impl TableSpec {
    pub fn new(
        name: impl Into<String>,
        owner: Owner,
        columns: Vec<Column>,
        keys: Vec<LogicalKey>,
        enums: BTreeMap<String, BTreeSet<String>>,
    ) -> Result<Self, SchemaError> {
        let name = name.into();
        let names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
        let unique: BTreeSet<&str> = names.iter().copied().collect();
        if unique.len() != names.len() {
            return Err(SchemaError::DuplicateColumn { relation: name });
        }
        for key in &keys {
            for column in &key.columns {
                if !names.contains(&column.as_str()) {
                    return Err(SchemaError::KeyColumnMissing {
                        relation: name,
                        column: column.clone(),
                    });
                }
            }
        }
        for column in enums.keys() {
            if !names.contains(&column.as_str()) {
                return Err(SchemaError::EnumColumnMissing {
                    relation: name,
                    column: column.clone(),
                });
            }
        }
        Ok(Self {
            name,
            owner,
            columns,
            keys,
            enums,
        })
    }

    #[must_use]
    pub fn column_names(&self) -> Vec<&str> {
        self.columns.iter().map(|c| c.name.as_str()).collect()
    }
}

// --------------------------------------------------------------------------- //
// closed vocabularies — every `in {…}` domain the S5 DDL declares
// --------------------------------------------------------------------------- //

/// The eleven promoted counters of S5.2 (a CLOSED, ordered set of typed
/// `run_stages` columns). `run_stages` builds its counter columns from this.
pub const PROMOTED_COUNTERS: [&str; 11] = [
    "rows_in",
    "rows_out",
    "candidate_pairs",
    "pairs_above_auto_merge",
    "entities_created",
    "entities_merged",
    "entities_split",
    "entities_retired",
    "edges_cut",
    "review_queue_added",
    "duration_ms",
];

/// The four relations carrying `rec_a_key < rec_b_key` (S5.0, D9).
pub const PAIR_RELATIONS: [&str; 4] = ["match_scores", "assertions", "review_queue", "cut_edges"];

pub const EVENT_TYPES: [&str; 7] = [
    "created",
    "member_added",
    "member_removed",
    "merged",
    "split",
    "retired",
    "edge_cut",
];
pub const ENTITY_STATUSES: [&str; 3] = ["active", "merged", "retired"];
pub const ASSERTION_KINDS: [&str; 2] = ["always", "never"];
pub const REVIEW_SUBJECT_TYPES: [&str; 2] = ["pair", "entity"];
pub const REVIEW_REASONS: [&str; 3] = ["gray_band", "never_unsatisfiable", "coherence"];
pub const REVIEW_STATUSES: [&str; 4] = ["open", "resolved_match", "resolved_no_match", "dismissed"];
pub const MODEL_STATUSES: [&str; 2] = ["active", "superseded"];
pub const RUN_MODES: [&str; 8] = [
    "incremental",
    "full",
    "train",
    "init",
    "maintain",
    "reset",
    "correction_pass",
    "stage",
];
pub const RUN_STAGES: [&str; 9] = [
    "init",
    "ingest",
    "standardize",
    "train",
    "match",
    "reconcile",
    "assemble",
    "maintain",
    "reset",
];
pub const RUN_STATUSES: [&str; 3] = ["running", "succeeded", "failed"];
pub const REBUILD_REASONS: [&str; 5] = [
    "std_version_bump",
    "survivorship_version_bump",
    "correction_pass",
    "operator",
    "lexicon_change",
];
pub const DISPOSITIONS: [&str; 2] = ["rebuild", "retire"];

// --------------------------------------------------------------------------- //
// the relations, in S5 declaration order (DDL-owned; DBT specs join next wake)
// --------------------------------------------------------------------------- //

/// The `CREATE TABLE IF NOT EXISTS` statement for a DDL-owned relation (S5.0);
/// pure text, `NOT NULL` the only constraint. Errors for a dbt-owned relation.
pub fn create_table_sql(spec: &TableSpec) -> Result<String, SchemaError> {
    if spec.owner != Owner::Ddl {
        return Err(SchemaError::DbtOwnedDdl {
            relation: spec.name.clone(),
        });
    }
    let body = spec
        .columns
        .iter()
        .map(|c| format!("  {}", c.sql()))
        .collect::<Vec<_>>()
        .join(",\n");
    Ok(format!(
        "CREATE TABLE IF NOT EXISTS {SCHEMA_QUALIFIER}.{} (\n{body}\n);",
        spec.name
    ))
}

fn col(name: &str, type_: &str) -> Column {
    Column::new(name, type_, true).expect("valid S5 type")
}
fn nn(name: &str, type_: &str) -> Column {
    Column::not_null(name, type_).expect("valid S5 type")
}
fn key(cols: &[&str]) -> LogicalKey {
    LogicalKey {
        columns: cols.iter().map(|s| (*s).to_string()).collect(),
        r#where: None,
    }
}
fn key_where(cols: &[&str], filter: &str) -> LogicalKey {
    LogicalKey {
        columns: cols.iter().map(|s| (*s).to_string()).collect(),
        r#where: Some(filter.to_string()),
    }
}
fn enum_map(pairs: &[(&str, &[&str])]) -> BTreeMap<String, BTreeSet<String>> {
    pairs
        .iter()
        .map(|(column, values)| {
            (
                (*column).to_string(),
                values.iter().map(|v| (*v).to_string()).collect(),
            )
        })
        .collect()
}
fn tspec(
    name: &str,
    columns: Vec<Column>,
    keys: Vec<LogicalKey>,
    enums: BTreeMap<String, BTreeSet<String>>,
) -> TableSpec {
    TableSpec::new(name, Owner::Ddl, columns, keys, enums).expect("valid DDL spec")
}

fn ddl_specs() -> Vec<TableSpec> {
    let mut run_stages_cols = vec![
        nn("run_id", VARCHAR),
        nn("stage", VARCHAR),
        nn("seq", BIGINT),
        nn("status", VARCHAR),
        nn("started_at", TIMESTAMP),
        col("ended_at", TIMESTAMP),
        col("snapshot_start", BIGINT),
        col("snapshot_end", BIGINT),
    ];
    run_stages_cols.extend(PROMOTED_COUNTERS.iter().map(|p| col(p, BIGINT)));
    run_stages_cols.extend([
        col("counters", JSON),
        col("error_class", VARCHAR),
        col("error_detail", VARCHAR),
    ]);

    vec![
        tspec(
            "raw_records",
            vec![
                nn("source_system", VARCHAR),
                nn("source_record_id", VARCHAR),
                nn("payload", JSON),
                nn("content_hash", VARCHAR),
                nn("is_deleted", BOOLEAN),
                col("deleted_at", TIMESTAMP),
                nn("ingest_batch_id", VARCHAR),
                nn("ingested_at", TIMESTAMP),
            ],
            vec![key(&["source_system", "source_record_id", "content_hash"])],
            BTreeMap::new(),
        ),
        tspec(
            "match_scores",
            vec![
                nn("rec_a_key", VARCHAR),
                nn("rec_b_key", VARCHAR),
                nn("match_probability", DOUBLE),
                nn("model_version", VARCHAR),
                nn("tf_snapshot_id", VARCHAR),
                nn("rec_a_content_hash", VARCHAR),
                nn("rec_b_content_hash", VARCHAR),
                nn("evidence", JSON),
                nn("is_active", BOOLEAN),
                col("invalidated_at", TIMESTAMP),
                col("invalidated_run_id", VARCHAR),
                nn("run_id", VARCHAR),
                nn("scored_at", TIMESTAMP),
            ],
            vec![key(&[
                "rec_a_key",
                "rec_b_key",
                "model_version",
                "tf_snapshot_id",
            ])],
            BTreeMap::new(),
        ),
        tspec(
            "entity_membership",
            vec![
                nn("source_system", VARCHAR),
                nn("source_record_id", VARCHAR),
                nn("record_key", VARCHAR),
                nn("entity_id", VARCHAR),
                nn("assigned_at", TIMESTAMP),
                nn("run_id", VARCHAR),
            ],
            vec![key(&["source_system", "source_record_id"])],
            BTreeMap::new(),
        ),
        tspec(
            "entities",
            vec![
                nn("entity_id", VARCHAR),
                nn("status", VARCHAR),
                col("merged_into", VARCHAR),
                nn("created_at", TIMESTAMP),
                nn("updated_at", TIMESTAMP),
                nn("created_run_id", VARCHAR),
                nn("updated_run_id", VARCHAR),
            ],
            vec![key(&["entity_id"])],
            enum_map(&[("status", &ENTITY_STATUSES[..])]),
        ),
        tspec(
            "entity_events",
            vec![
                nn("event_id", VARCHAR),
                nn("seq", BIGINT),
                nn("run_id", VARCHAR),
                nn("entity_id", VARCHAR),
                nn("event_type", VARCHAR),
                nn("details", JSON),
                nn("details_hash", VARCHAR),
                nn("occurred_at", TIMESTAMP),
            ],
            vec![
                key(&["event_id"]),
                key(&["run_id", "entity_id", "event_type", "details_hash"]),
            ],
            enum_map(&[("event_type", &EVENT_TYPES[..])]),
        ),
        tspec(
            "assertions",
            vec![
                nn("assertion_id", VARCHAR),
                nn("rec_a_key", VARCHAR),
                nn("rec_b_key", VARCHAR),
                nn("kind", VARCHAR),
                nn("active", BOOLEAN),
                nn("created_by", VARCHAR),
                nn("created_at", TIMESTAMP),
                col("retracted_by", VARCHAR),
                col("retracted_at", TIMESTAMP),
                col("note", VARCHAR),
            ],
            vec![
                key(&["assertion_id"]),
                key_where(&["rec_a_key", "rec_b_key"], "active"),
            ],
            enum_map(&[("kind", &ASSERTION_KINDS[..])]),
        ),
        tspec(
            "review_queue",
            vec![
                nn("review_id", VARCHAR),
                nn("subject_type", VARCHAR),
                col("rec_a_key", VARCHAR),
                col("rec_b_key", VARCHAR),
                col("entity_id", VARCHAR),
                nn("reason", VARCHAR),
                col("match_probability", DOUBLE),
                col("waterfall", JSON),
                nn("status", VARCHAR),
                nn("first_seen_run_id", VARCHAR),
                nn("last_seen_run_id", VARCHAR),
                col("resolved_by", VARCHAR),
                col("resolved_at", TIMESTAMP),
            ],
            vec![
                key(&["review_id"]),
                key_where(
                    &[
                        "subject_type",
                        "rec_a_key",
                        "rec_b_key",
                        "entity_id",
                        "reason",
                    ],
                    "status='open'",
                ),
            ],
            enum_map(&[
                ("subject_type", &REVIEW_SUBJECT_TYPES[..]),
                ("reason", &REVIEW_REASONS[..]),
                ("status", &REVIEW_STATUSES[..]),
            ]),
        ),
        tspec(
            "model_registry",
            vec![
                nn("model_version", VARCHAR),
                nn("status", VARCHAR),
                nn("trained_at", TIMESTAMP),
                nn("corpus_snapshot", BIGINT),
                nn("params_path", VARCHAR),
                nn("tf_tables_path", VARCHAR),
                nn("tf_snapshot_id", VARCHAR),
                nn("config_hash", VARCHAR),
                nn("metrics", JSON),
                nn("run_id", VARCHAR),
            ],
            vec![key(&["model_version"]), key_where(&[], "status='active'")],
            enum_map(&[("status", &MODEL_STATUSES[..])]),
        ),
        tspec(
            "tf_lookup",
            vec![
                nn("model_version", VARCHAR),
                nn("tf_snapshot_id", VARCHAR),
                nn("column_name", VARCHAR),
                nn("value", VARCHAR),
                nn("tf_value", DOUBLE),
            ],
            vec![key(&[
                "model_version",
                "tf_snapshot_id",
                "column_name",
                "value",
            ])],
            BTreeMap::new(),
        ),
        tspec(
            "cut_edges",
            vec![
                nn("cut_id", VARCHAR),
                nn("rec_a_key", VARCHAR),
                nn("rec_b_key", VARCHAR),
                nn("match_probability", DOUBLE),
                col("model_version", VARCHAR),
                col("tf_snapshot_id", VARCHAR),
                nn("assertion_id", VARCHAR),
                nn("active", BOOLEAN),
                nn("cut_run_id", VARCHAR),
                nn("cut_at", TIMESTAMP),
                col("released_run_id", VARCHAR),
                col("released_at", TIMESTAMP),
            ],
            vec![
                key(&["cut_id"]),
                key_where(&["rec_a_key", "rec_b_key"], "active"),
            ],
            BTreeMap::new(),
        ),
        tspec(
            "runs",
            vec![
                nn("run_id", VARCHAR),
                nn("tenant", VARCHAR),
                nn("mode", VARCHAR),
                nn("status", VARCHAR),
                nn("started_at", TIMESTAMP),
                col("ended_at", TIMESTAMP),
                nn("config_hash", VARCHAR),
                col("model_version", VARCHAR),
                col("tf_snapshot_id", VARCHAR),
                nn("std_version", VARCHAR),
                nn("survivorship_version", VARCHAR),
                col("lexicon_hash", VARCHAR),
                nn("code_version", VARCHAR),
                col("rebuild_reason", VARCHAR),
                col("snapshot_start", BIGINT),
                col("snapshot_end", BIGINT),
            ],
            vec![key(&["run_id"])],
            enum_map(&[
                ("mode", &RUN_MODES[..]),
                ("status", &RUN_STATUSES[..]),
                ("rebuild_reason", &REBUILD_REASONS[..]),
            ]),
        ),
        tspec(
            "run_stages",
            run_stages_cols,
            vec![key(&["run_id", "seq"]), key(&["run_id", "stage"])],
            enum_map(&[("stage", &RUN_STAGES[..]), ("status", &RUN_STATUSES[..])]),
        ),
        tspec(
            "ingest_batches",
            vec![
                nn("ingest_batch_id", VARCHAR),
                nn("run_id", VARCHAR),
                nn("source_system", VARCHAR),
                nn("path", VARCHAR),
                nn("new_count", BIGINT),
                nn("changed_count", BIGINT),
                nn("unchanged_count", BIGINT),
                nn("tombstone_count", BIGINT),
                nn("resurrected_count", BIGINT),
                nn("full_refresh_keys", BOOLEAN),
                nn("created_at", TIMESTAMP),
            ],
            vec![key(&["ingest_batch_id"])],
            BTreeMap::new(),
        ),
        tspec(
            "er_standardize_work",
            vec![
                nn("run_id", VARCHAR),
                nn("source_system", VARCHAR),
                nn("ingest_batch_id", VARCHAR),
                nn("full_refresh", BOOLEAN),
            ],
            vec![key(&["run_id", "source_system", "ingest_batch_id"])],
            BTreeMap::new(),
        ),
        tspec(
            "er_touched_entities",
            vec![
                nn("run_id", VARCHAR),
                nn("entity_id", VARCHAR),
                nn("disposition", VARCHAR),
                nn("created_at", TIMESTAMP),
            ],
            vec![key(&["run_id", "entity_id"])],
            enum_map(&[("disposition", &DISPOSITIONS[..])]),
        ),
        tspec(
            "nickname_variants",
            vec![
                nn("variant_id", VARCHAR),
                nn("variant_a", VARCHAR),
                nn("variant_b", VARCHAR),
                nn("active", BOOLEAN),
                nn("created_by", VARCHAR),
                nn("created_at", TIMESTAMP),
                col("retracted_by", VARCHAR),
                col("retracted_at", TIMESTAMP),
            ],
            vec![
                key(&["variant_id"]),
                key_where(&["variant_a", "variant_b"], "active"),
            ],
            BTreeMap::new(),
        ),
        // Steward golden field overrides (mass-update): one latest-wins row per
        // (entity_id, attribute); `value` NULL means the steward cleared the
        // field. Applied as a post-assembly UPDATE over golden_records, so an
        // empty table is a byte-exact no-op (the determinism contract holds).
        tspec(
            "golden_overrides",
            vec![
                nn("entity_id", VARCHAR),
                nn("attribute", VARCHAR),
                col("value", VARCHAR),
                nn("created_by", VARCHAR),
                nn("created_at", TIMESTAMP),
                nn("source_action_id", VARCHAR),
            ],
            vec![key(&["entity_id", "attribute"])],
            BTreeMap::new(),
        ),
        // Steward record overlays (PR 1.3): protect/suppress one record. One
        // latest-wins active row per (record_key, kind). `suppress` drops the
        // record from reconcile clustering; `protect` forces it master on the
        // read path. Loaded only when the table exists, so an empty/absent
        // overlay is a byte-exact no-op (the determinism contract holds).
        tspec(
            "record_overlays",
            vec![
                nn("record_key", VARCHAR),
                nn("kind", VARCHAR),
                nn("active", BOOLEAN),
                nn("created_by", VARCHAR),
                nn("created_at", TIMESTAMP),
                nn("source_action_id", VARCHAR),
                col("retracted_at", TIMESTAMP),
            ],
            vec![key(&["record_key", "kind"])],
            BTreeMap::new(),
        ),
    ]
}

/// The standardized payload S5 declares once for `stg_records` and reuses for
/// `int_std_records` (which prefixes `record_key`).
fn std_payload() -> Vec<Column> {
    vec![
        nn("source_system", VARCHAR),
        nn("source_record_id", VARCHAR),
        nn("content_hash", VARCHAR),
        nn("std_version", VARCHAR),
        col("given_name", VARCHAR),
        col("family_name", VARCHAR),
        nn("name_variants", LIST_VARCHAR),
        col("email", VARCHAR),
        col("email_valid", BOOLEAN),
        col("phone_e164", VARCHAR),
        col("phone_valid", BOOLEAN),
        col("addr_number", VARCHAR),
        col("addr_street", VARCHAR),
        col("addr_unit", VARCHAR),
        col("addr_city", VARCHAR),
        col("addr_region", VARCHAR),
        col("addr_postal", VARCHAR),
        col("birth_date", DATE),
        col("metadata", JSON),
        col("updated_at_source", TIMESTAMP),
        nn("ingest_batch_id", VARCHAR),
        nn("ingested_at", TIMESTAMP),
    ]
}

/// `golden_records`' survivable columns: names from `columns.rs`, types from the
/// std payload (never re-listed), all nullable.
fn golden_survivable() -> Vec<Column> {
    let payload = std_payload();
    GOLDEN_SURVIVABLE_COLUMNS
        .iter()
        .map(|name| {
            let type_ = payload
                .iter()
                .find(|c| c.name == *name)
                .expect("survivable column is in the std payload")
                .r#type
                .clone();
            Column::new(*name, type_, true).expect("valid type")
        })
        .collect()
}

/// The standardized payload for the ACCOUNT entity type (the Account analogue of
/// [`std_payload`]): the same structural columns, the Account canonical attributes
/// in place of the person ones. Account's `stg_records`/`int_std_records` reuse
/// it; the person payload is untouched. The standardize stage that populates it
/// lands in a later sub-PR (1.1b).
fn acct_std_payload() -> Vec<Column> {
    vec![
        nn("source_system", VARCHAR),
        nn("source_record_id", VARCHAR),
        nn("content_hash", VARCHAR),
        nn("std_version", VARCHAR),
        col("company_name", VARCHAR),
        col("company_name_norm", VARCHAR),
        nn("company_variants", LIST_VARCHAR),
        col("domain", VARCHAR),
        col("domain_norm", VARCHAR),
        col("phone_e164", VARCHAR),
        col("phone_valid", BOOLEAN),
        col("addr_street", VARCHAR),
        col("addr_city", VARCHAR),
        col("addr_region", VARCHAR),
        col("addr_postal", VARCHAR),
        col("metadata", JSON),
        col("updated_at_source", TIMESTAMP),
        nn("ingest_batch_id", VARCHAR),
        nn("ingested_at", TIMESTAMP),
    ]
}

/// The `int_std_records` columns for an entity type: `record_key` plus the
/// standardized payload a tenant config's blocking / comparison SQL reads
/// (person via [`std_payload`], account via [`acct_std_payload`]). This is the
/// single source for the publish-time config-reference check (`er-server`'s
/// `config_check`), so there is no hand-maintained duplicate to drift.
#[must_use]
pub fn int_std_record_columns(account: bool) -> Vec<Column> {
    let mut cols = vec![nn("record_key", VARCHAR)];
    cols.extend(if account {
        acct_std_payload()
    } else {
        std_payload()
    });
    cols
}

/// The Account `golden_records` survivable columns (the Account analogue of
/// [`golden_survivable`]): the raw canonical Account attributes, all nullable,
/// types from the Account std payload.
fn acct_golden_survivable() -> Vec<Column> {
    const ACCT_SURVIVABLE: [&str; 7] = [
        "company_name",
        "domain",
        "phone_e164",
        "addr_street",
        "addr_city",
        "addr_region",
        "addr_postal",
    ];
    let payload = acct_std_payload();
    ACCT_SURVIVABLE
        .iter()
        .map(|name| {
            let type_ = payload
                .iter()
                .find(|c| c.name == *name)
                .expect("survivable column is in the account std payload")
                .r#type
                .clone();
            Column::new(*name, type_, true).expect("valid type")
        })
        .collect()
}

fn dbt_specs() -> Vec<TableSpec> {
    let dspec = |name: &str, columns, keys, enums| {
        TableSpec::new(name, Owner::Dbt, columns, keys, enums).expect("valid dbt spec")
    };

    let mut int_std_cols = vec![nn("record_key", VARCHAR)];
    int_std_cols.extend(std_payload());

    let mut golden_cols = vec![nn("entity_id", VARCHAR)];
    golden_cols.extend(golden_survivable());
    golden_cols.extend([
        col("metadata", JSON),
        nn("survivorship_version", VARCHAR),
        nn("assembled_at", TIMESTAMP),
    ]);

    // The parallel Account entity-type relations (1.1a). Same shapes as the
    // person std/golden relations, over the Account attributes. Account records
    // share the entity-agnostic DDL tables (raw_records, match_scores, entities,
    // …); only these standardized/golden projections are entity-specific.
    let mut acct_int_std_cols = vec![nn("record_key", VARCHAR)];
    acct_int_std_cols.extend(acct_std_payload());

    let mut acct_golden_cols = vec![nn("entity_id", VARCHAR)];
    acct_golden_cols.extend(acct_golden_survivable());
    acct_golden_cols.extend([
        col("metadata", JSON),
        nn("survivorship_version", VARCHAR),
        nn("assembled_at", TIMESTAMP),
    ]);

    vec![
        dspec(
            "stg_records",
            std_payload(),
            vec![key(&["source_system", "source_record_id", "content_hash"])],
            BTreeMap::new(),
        ),
        dspec(
            "int_std_records",
            int_std_cols,
            vec![key(&["record_key"])],
            BTreeMap::new(),
        ),
        dspec(
            "int_blocking_keys",
            vec![
                nn("key_type", VARCHAR),
                nn("key_value", VARCHAR),
                nn("record_key", VARCHAR),
                nn("source_system", VARCHAR),
                nn("source_record_id", VARCHAR),
            ],
            vec![key(&["key_type", "key_value", "record_key"])],
            BTreeMap::new(),
        ),
        dspec(
            "golden_records",
            golden_cols,
            vec![key(&["entity_id"])],
            BTreeMap::new(),
        ),
        dspec(
            "golden_lineage",
            vec![
                nn("entity_id", VARCHAR),
                nn("attribute", VARCHAR),
                nn("record_key", VARCHAR),
                nn("source_system", VARCHAR),
                nn("source_record_id", VARCHAR),
                nn("rule", VARCHAR),
                nn("survivorship_version", VARCHAR),
                nn("assembled_at", TIMESTAMP),
            ],
            vec![key(&["entity_id", "attribute"])],
            enum_map(&[("attribute", GOLDEN_LINEAGE_ATTRIBUTES.as_slice())]),
        ),
        dspec(
            "golden_display",
            vec![
                nn("entity_id", VARCHAR),
                col("display_name", VARCHAR),
                col("display_email", VARCHAR),
                col("display_phone", VARCHAR),
                col("display_address", VARCHAR),
                nn("assembled_at", TIMESTAMP),
            ],
            vec![key(&["entity_id"])],
            BTreeMap::new(),
        ),
        // --- Account entity-type relations (1.1a) ---
        dspec(
            "acct_stg_records",
            acct_std_payload(),
            vec![key(&["source_system", "source_record_id", "content_hash"])],
            BTreeMap::new(),
        ),
        dspec(
            "acct_int_std_records",
            acct_int_std_cols,
            vec![key(&["record_key"])],
            BTreeMap::new(),
        ),
        dspec(
            "acct_golden_records",
            acct_golden_cols,
            vec![key(&["entity_id"])],
            BTreeMap::new(),
        ),
        dspec(
            "acct_golden_lineage",
            vec![
                nn("entity_id", VARCHAR),
                nn("attribute", VARCHAR),
                nn("record_key", VARCHAR),
                nn("source_system", VARCHAR),
                nn("source_record_id", VARCHAR),
                nn("rule", VARCHAR),
                nn("survivorship_version", VARCHAR),
                nn("assembled_at", TIMESTAMP),
            ],
            vec![key(&["entity_id", "attribute"])],
            enum_map(&[(
                "attribute",
                ["address", "company_name", "domain", "phone_e164"].as_slice(),
            )]),
        ),
        dspec(
            "acct_golden_display",
            vec![
                nn("entity_id", VARCHAR),
                col("display_name", VARCHAR),
                col("display_domain", VARCHAR),
                col("display_phone", VARCHAR),
                col("display_address", VARCHAR),
                nn("assembled_at", TIMESTAMP),
            ],
            vec![key(&["entity_id"])],
            BTreeMap::new(),
        ),
    ]
}

/// Every relation of S5, in declaration order (DDL specs then DBT specs).
#[must_use]
pub fn all_specs() -> Vec<TableSpec> {
    let mut specs = ddl_specs();
    specs.extend(dbt_specs());
    specs
}

/// The DDL-owned relation names, in declaration order.
#[must_use]
pub fn ddl_owned() -> Vec<String> {
    all_specs()
        .into_iter()
        .filter(|s| s.owner == Owner::Ddl)
        .map(|s| s.name)
        .collect()
}

/// The dbt-owned relation names, in declaration order.
#[must_use]
pub fn dbt_owned() -> Vec<String> {
    all_specs()
        .into_iter()
        .filter(|s| s.owner == Owner::Dbt)
        .map(|s| s.name)
        .collect()
}

/// The logical keys S5.0 declares for `relation`, or `None` if not a relation.
#[must_use]
pub fn logical_key(relation: &str) -> Option<Vec<LogicalKey>> {
    all_specs()
        .into_iter()
        .find(|s| s.name == relation)
        .map(|s| s.keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_and_column_sql() {
        assert_eq!(COLUMN_TYPES.len(), 8);
        assert!(is_valid_type("VARCHAR") && is_valid_type("LIST(VARCHAR)"));
        assert!(!is_valid_type("ARRAY"));
        assert_eq!(
            Column::not_null("x", VARCHAR).unwrap().sql(),
            "x VARCHAR NOT NULL"
        );
        assert_eq!(Column::new("y", JSON, true).unwrap().sql(), "y JSON");
        assert!(matches!(
            Column::new("z", "INT", true),
            Err(SchemaError::BadType { .. })
        ));
    }

    #[test]
    fn owner_spellings() {
        assert_eq!(Owner::Ddl.as_str(), "ddl.py");
        assert_eq!(Owner::Dbt.as_str(), "dbt");
    }

    #[test]
    fn tablespec_validation() {
        let col = |n: &str| Column::new(n, VARCHAR, true).unwrap();
        // Happy path.
        let spec = TableSpec::new(
            "t",
            Owner::Ddl,
            vec![col("a"), col("b")],
            vec![LogicalKey {
                columns: vec!["a".into()],
                r#where: None,
            }],
            BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(spec.column_names(), vec!["a", "b"]);
        // Duplicate column.
        assert!(matches!(
            TableSpec::new(
                "t",
                Owner::Ddl,
                vec![col("a"), col("a")],
                vec![],
                BTreeMap::new()
            ),
            Err(SchemaError::DuplicateColumn { .. })
        ));
        // Key column not present.
        assert!(matches!(
            TableSpec::new(
                "t",
                Owner::Ddl,
                vec![col("a")],
                vec![LogicalKey {
                    columns: vec!["missing".into()],
                    r#where: None
                }],
                BTreeMap::new(),
            ),
            Err(SchemaError::KeyColumnMissing { .. })
        ));
        // Enum column not present.
        let mut enums = BTreeMap::new();
        enums.insert("nope".to_string(), BTreeSet::from(["x".to_string()]));
        assert!(matches!(
            TableSpec::new("t", Owner::Ddl, vec![col("a")], vec![], enums),
            Err(SchemaError::EnumColumnMissing { .. })
        ));
    }

    #[test]
    fn vocabulary_sizes_and_order() {
        assert_eq!(PROMOTED_COUNTERS.len(), 11);
        assert_eq!(PROMOTED_COUNTERS[0], "rows_in");
        assert_eq!(PROMOTED_COUNTERS[10], "duration_ms");
        assert_eq!(PAIR_RELATIONS.len(), 4);
        assert_eq!(EVENT_TYPES.len(), 7);
        assert_eq!(ENTITY_STATUSES.len(), 3);
        assert_eq!(ASSERTION_KINDS.len(), 2);
        assert_eq!(REVIEW_REASONS.len(), 3);
        assert_eq!(REVIEW_STATUSES.len(), 4);
        assert_eq!(RUN_MODES.len(), 8);
        assert_eq!(RUN_STAGES.len(), 9);
        assert_eq!(RUN_STATUSES.len(), 3);
        assert_eq!(REBUILD_REASONS.len(), 5);
        assert_eq!(DISPOSITIONS.len(), 2);
        assert!(EVENT_TYPES.contains(&"edge_cut"));
        assert!(RUN_STAGES.contains(&"reconcile"));
    }
}
