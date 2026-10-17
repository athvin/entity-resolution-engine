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
mod emit;
mod plan;
mod rewrite;
mod stage;
mod statement;

pub use analyze::{Diagnostic, analyze_catalog, analyze_statement};
pub use catalog::build_store_catalog;
pub use emit::emit_query_modules;
pub use plan::parse_and_plan;
pub use rewrite::{
    Rewritten, rewrite_json_casts, rewrite_lake, rewrite_map_subscript, rewrite_metadata_lambdas,
    strip_named_args,
};
pub use stage::build_lake_stage_catalog;
pub use statement::{
    LoadedStatement, Mode, Operation, Pin, ReturnColumn, Returns, StatementParam, StmtError,
    load_statements, operation_for_folder,
};
