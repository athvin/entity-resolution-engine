//! Standardization primitives (port of `src/er/std/*`, DesignDoc.md S4.2).
//!
//! The standardization *stage* runs as dbt SQL macros; the Python classes these
//! port are the macros' oracles. Reproduced here in Rust so the engine owns the
//! grammar directly. DuckDB's regex is RE2 — the same engine Rust's `regex` crate
//! implements — so the patterns are shared verbatim.
//!
//! - [`address`]: the S4.2 `address_parse` v1 grammar.
//! - [`lexicon`]: the nickname-lexicon canonicalisation and `lexicon_hash`.

pub mod address;
pub mod lexicon;
pub mod sql;
