//! The parsed `entity.yaml` model: one [`Entity`] per table, its [`Column`]s in
//! declaration order, and the [`GenerateBlock`] that says what to emit for it.

use serde::Deserialize;

use crate::types::{ColumnType, Language, Store, Wire};

/// The `generate` block: which stores get DDL, which languages get readers, and
/// which wires get a schema. There is no default — an entity with no block is a
/// spec error (enforced by this being a non-`Option` field), because a silent
/// guess is exactly what the framework refuses.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerateBlock {
    /// Any of `duckdb`, `postgres` — decides which DDL is rendered.
    pub stores: Vec<Store>,
    /// Any of `rust`, `typescript` — decides which readers are emitted.
    pub languages: Vec<Language>,
    /// Any of `arrow`, `json` — decides whether `arrow_schema()` / OpenAPI emit.
    #[serde(default)]
    pub wire: Vec<Wire>,
}

/// A referential expectation for a control-plane column (`postgres` only).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    /// The relation the foreign key points at.
    pub relation: String,
    /// The column in that relation.
    pub column: String,
}

/// One column of an entity, in declaration order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Column {
    /// The column name.
    pub name: String,
    /// One of the eight entity types.
    #[serde(rename = "type")]
    pub col_type: ColumnType,
    /// Whether the column admits NULL. Columns default to nullable; a `NOT NULL`
    /// column says so explicitly, matching the lake registry's constraint model.
    #[serde(default = "default_true")]
    pub nullable: bool,
    /// Required prose: this tree is also the documentation of every column.
    pub description: String,
    /// A volatile column (e.g. an assembly timestamp) excluded from determinism
    /// comparisons. It is still part of the entity's shape.
    #[serde(default)]
    pub volatile: bool,
    /// Control-plane only: a SQL default expression.
    #[serde(default)]
    pub default: Option<String>,
    /// Control-plane only: render this `bigint` column as `bigserial` (an
    /// auto-incrementing identity). Affects the Postgres DDL *only* — the column
    /// type stays `bigint` for the struct, the Arrow/JSON wire and the differ
    /// (which `information_schema` also reports as `bigint`).
    #[serde(default)]
    pub serial: bool,
    /// Control-plane only: a foreign-key reference.
    #[serde(default)]
    pub references: Option<Reference>,
    /// Control-plane only: the closed set of values this column may take
    /// (rendered as a CHECK constraint and used by E010).
    #[serde(default)]
    pub accepted_values: Option<Vec<String>>,
}

fn default_true() -> bool {
    true
}

/// A secondary index for a control-plane entity (`postgres` only).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Index {
    /// The index name (Postgres indexes are named objects).
    pub name: String,
    /// The indexed columns, in order.
    pub columns: Vec<String>,
    /// Whether the index is unique.
    #[serde(default)]
    pub unique: bool,
    /// An optional partial-index predicate.
    #[serde(default)]
    pub r#where: Option<String>,
}

/// One entity: a table and everything generated from it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entity {
    /// Must be the literal `entity` (validated on load).
    pub kind: String,
    /// The schema version; bumped only on a breaking change.
    pub version: u64,
    /// The entity name — the Rust module and query namespace (e.g. `golden_record`).
    pub name: String,
    /// The relation (table) name (e.g. `golden_records`).
    pub relation: String,
    /// Required prose describing the table.
    pub description: String,
    /// The logical key columns.
    pub key: Vec<String>,
    /// What to generate for this entity.
    pub generate: GenerateBlock,
    /// The columns, in declaration order.
    pub columns: Vec<Column>,
    /// Control-plane only: secondary indexes.
    #[serde(default)]
    pub indexes: Vec<Index>,
}

impl Entity {
    /// The declaration-ordered column names.
    #[must_use]
    pub fn column_names(&self) -> Vec<&str> {
        self.columns.iter().map(|c| c.name.as_str()).collect()
    }

    /// Find a column by name.
    #[must_use]
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// Whether this entity is generated for the given store.
    #[must_use]
    pub fn has_store(&self, store: Store) -> bool {
        self.generate.stores.contains(&store)
    }

    /// Whether this entity is generated for the given language.
    #[must_use]
    pub fn has_language(&self, language: Language) -> bool {
        self.generate.languages.contains(&language)
    }

    /// Whether this entity is generated for the given wire.
    #[must_use]
    pub fn has_wire(&self, wire: Wire) -> bool {
        self.generate.wire.contains(&wire)
    }

    /// The store-qualified relation key for a given store (`lake.main.golden_records`,
    /// `public.jobs`) — the catalog's lookup key and the analyser's table name.
    #[must_use]
    pub fn qualified_name(&self, store: Store) -> String {
        format!("{}.{}", store.qualifier(), self.relation)
    }
}
