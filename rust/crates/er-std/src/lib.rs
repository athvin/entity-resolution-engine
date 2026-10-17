//! Standardization primitives (port of `src/er/std/*`, DesignDoc.md S4.2).
//!
//! In the retired Python/dbt implementation the standardization *stage* ran as
//! dbt SQL macros with these classes as their oracles. Reproduced here in Rust
//! so the engine owns the grammar directly. DuckDB's regex is RE2 — the same engine Rust's `regex` crate
//! implements — so the patterns are shared verbatim.
//!
//! - [`address`]: the S4.2 `address_parse` v1 grammar.
//! - [`lexicon`]: the nickname-lexicon canonicalisation and `lexicon_hash`.

pub mod address;
pub mod lexicon;
pub mod sql;
