//! `er-sql-check` — the static analyser.
//!
//! Loads the statement and function YAML, rewrites what DataFusion cannot plan,
//! plans each statement against a two-store catalog built from the entities
//! (`ff-analysis`), runs the E0xx passes, and emits the typed query functions and
//! the oracle. This first slice proves the catalog mechanism: an entity catalog
//! becomes an `ff_analysis::SchemaCatalog` keyed by store-qualified relation name,
//! ready for the planner.

mod analyze;
mod catalog;
mod plan;
mod rewrite;
mod statement;

pub use analyze::{Diagnostic, analyze_catalog, analyze_statement};
pub use catalog::build_store_catalog;
pub use plan::parse_and_plan;
pub use rewrite::{Rewritten, rewrite_lake};
pub use statement::{
    LoadedStatement, Mode, Operation, Returns, StatementParam, StmtError, load_statements,
    operation_for_folder,
};
