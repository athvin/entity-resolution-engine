//! The S5 schema registry (Rust port of `src/er/lake/*`), dependency-free so
//! `config`/`golden`/tests can consume it without pulling a DB into their graph.
//!
//! - [`columns`]: the ordered column sets S5.0 requires to have one definition.
//! - [`model`]: the relation/DDL registry (`TableSpec`, owners, DDL rendering).
//! - [`dbt_sources`]: `dbt/models/sources.yml` rendered from the registry.

pub mod columns;
pub mod dbt_sources;
pub mod model;
