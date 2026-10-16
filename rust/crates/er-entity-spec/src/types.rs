//! The closed type vocabulary: the eight types an entity column may be, and the
//! three `generate`-block axes (store, language, wire).
//!
//! The eight types are the intersection of what DuckLake, Postgres, Arrow, serde
//! and TypeScript all carry without loss. A ninth type is a spec error, not a
//! mapping to add — so the vocabulary is an enum, and an unknown spelling fails to
//! parse rather than falling through to a default.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer};

/// The eight — and only eight — types a declared entity column may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColumnType {
    /// A UTF-8 string.
    Varchar,
    /// A 64-bit signed integer.
    Bigint,
    /// A 64-bit IEEE float.
    Double,
    /// A boolean.
    Boolean,
    /// A calendar date, no time.
    Date,
    /// A timestamp; UTC by convention, naive on the lake, `timestamptz` on control.
    Timestamp,
    /// A JSON document.
    Json,
    /// A list of strings.
    ListVarchar,
}

impl ColumnType {
    /// The YAML spelling this type parses from and renders back to.
    #[must_use]
    pub fn as_yaml(self) -> &'static str {
        match self {
            ColumnType::Varchar => "varchar",
            ColumnType::Bigint => "bigint",
            ColumnType::Double => "double",
            ColumnType::Boolean => "boolean",
            ColumnType::Date => "date",
            ColumnType::Timestamp => "timestamp",
            ColumnType::Json => "json",
            ColumnType::ListVarchar => "list<varchar>",
        }
    }

    /// The DuckLake / DuckDB type string (`er-schema`'s vocabulary, by value).
    /// Returned as a `&str` so this crate stays free of an `er-schema` dependency;
    /// the emitter feeds it to `er_schema::create_table_sql`.
    #[must_use]
    pub fn ducklake_type(self) -> &'static str {
        match self {
            ColumnType::Varchar => "VARCHAR",
            ColumnType::Bigint => "BIGINT",
            ColumnType::Double => "DOUBLE",
            ColumnType::Boolean => "BOOLEAN",
            ColumnType::Date => "DATE",
            ColumnType::Timestamp => "TIMESTAMP",
            ColumnType::Json => "JSON",
            ColumnType::ListVarchar => "LIST(VARCHAR)",
        }
    }

    /// The Postgres type string for a control-plane column.
    #[must_use]
    pub fn postgres_type(self) -> &'static str {
        match self {
            ColumnType::Varchar => "text",
            ColumnType::Bigint => "bigint",
            ColumnType::Double => "double precision",
            ColumnType::Boolean => "boolean",
            ColumnType::Date => "date",
            ColumnType::Timestamp => "timestamptz",
            ColumnType::Json => "jsonb",
            ColumnType::ListVarchar => "text[]",
        }
    }

    /// The Rust type the generated struct field uses (before the nullable `Option`
    /// wrapper). One type for both stores, so a dual-store entity has one struct.
    #[must_use]
    pub fn rust_type(self) -> &'static str {
        match self {
            ColumnType::Varchar => "String",
            ColumnType::Bigint => "i64",
            ColumnType::Double => "f64",
            ColumnType::Boolean => "bool",
            ColumnType::Date => "::chrono::NaiveDate",
            ColumnType::Timestamp => "::chrono::DateTime<::chrono::Utc>",
            ColumnType::Json => "::serde_json::Value",
            ColumnType::ListVarchar => "Vec<String>",
        }
    }

    /// The TypeScript type the generated binding field uses (before `| null`).
    #[must_use]
    pub fn typescript_type(self) -> &'static str {
        match self {
            // i64 is `number`; the reader range-checks it.
            ColumnType::Bigint | ColumnType::Double => "number",
            ColumnType::Boolean => "boolean",
            // Dates and timestamps cross as ISO strings; JSON stays `unknown`.
            ColumnType::Varchar | ColumnType::Date | ColumnType::Timestamp => "string",
            ColumnType::Json => "unknown",
            ColumnType::ListVarchar => "string[]",
        }
    }
}

/// An unrecognised type spelling — the ninth-type spec error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{0:?} is not one of the eight entity types (varchar, bigint, double, boolean, date, timestamp, json, list<varchar>)"
)]
pub struct ParseTypeError(pub String);

impl FromStr for ColumnType {
    type Err = ParseTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // `list<varchar>` tolerates internal whitespace (`list< varchar >`); every
        // other spelling is exact and case-sensitive-lowercase by convention.
        let normalized: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        match normalized.as_str() {
            "varchar" => Ok(ColumnType::Varchar),
            "bigint" => Ok(ColumnType::Bigint),
            "double" => Ok(ColumnType::Double),
            "boolean" => Ok(ColumnType::Boolean),
            "date" => Ok(ColumnType::Date),
            "timestamp" => Ok(ColumnType::Timestamp),
            "json" => Ok(ColumnType::Json),
            "list<varchar>" => Ok(ColumnType::ListVarchar),
            _ => Err(ParseTypeError(s.to_string())),
        }
    }
}

impl fmt::Display for ColumnType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_yaml())
    }
}

impl<'de> Deserialize<'de> for ColumnType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// A persistence store an entity's DDL is rendered for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Store {
    /// The DuckLake analytical lake, planned with `DuckDbDialect`.
    Duckdb,
    /// The Postgres control plane, planned with `PostgreSqlDialect`.
    Postgres,
}

impl Store {
    /// The schema-qualified prefix the catalog keys this store's relations under.
    #[must_use]
    pub fn qualifier(self) -> &'static str {
        match self {
            Store::Duckdb => "lake.main",
            Store::Postgres => "public",
        }
    }
}

/// A target language a reader is emitted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// The `er-entities` crate: struct, `arrow_schema()`, query functions.
    Rust,
    /// The browser/BFF binding and Arrow reader.
    Typescript,
}

/// A wire format an entity's payloads cross on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Wire {
    /// Arrow IPC — `arrow_schema()` and the TypeScript reader are emitted.
    Arrow,
    /// JSON — the OpenAPI component schema is emitted.
    Json,
}
