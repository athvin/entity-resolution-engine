//! `er-sql-check` — the static analyser.
//!
//! Loads the statement and function YAML, rewrites what DataFusion cannot plan,
//! plans each statement against a two-store catalog built from the entities
//! (`ff-analysis`), runs the E0xx passes, and emits the typed query functions and
//! the oracle. This first slice proves the catalog mechanism: an entity catalog
//! becomes an `ff_analysis::SchemaCatalog` keyed by store-qualified relation name,
//! ready for the planner.

mod catalog;

pub use catalog::build_store_catalog;
