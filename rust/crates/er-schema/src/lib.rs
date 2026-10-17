//! The S5 schema registry (Rust port of `src/er/lake/*`), dependency-free so
//! `config`/`golden`/tests can consume it without pulling a DB into their graph.
//!
//! - [`columns`]: the ordered column sets S5.0 requires to have one definition.
//! - [`model`]: the relation/DDL registry (`TableSpec`, owners, DDL rendering).

pub mod columns;
pub mod model;
