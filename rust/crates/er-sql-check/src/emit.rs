//! The query emitter. For each planned statement it writes one typed Rust
//! function into the entity's generated module, namespaced by operation
//! (`golden_record::read::list_page`). Emitting the function is a text transform
//! of the validated `.sql` + its yaml — no planning — so it lives here beside the
//! statement model but needs no DataFusion at emit time.
//!
//! Lake reads run through `duckdb::Statement::query_arrow` and return
//! `arrow::record_batch::RecordBatch`. The snapshot is spliced into the statement
//! text (DuckLake does not accept a parameter inside `AT (VERSION => ..)`); every
//! other parameter is bound positionally as `$1`, `$2`, … in yaml order.

// A text emitter: `push_str(&format!(..))` reads more clearly than threading
// `write!` through every line.
#![allow(clippy::format_push_string)]

use std::collections::BTreeMap;

use er_entity_spec::{ColumnType, Entity, Store};
use regex::{NoExpand, Regex};

use crate::rewrite::rewrite_lake;
use crate::statement::{LoadedStatement, Operation, StatementParam};

/// The Rust argument type for a declared parameter (references for strings).
fn param_arg_type(param: &StatementParam) -> String {
    let base = match param.param_type {
        ColumnType::Varchar | ColumnType::Json => "&str",
        ColumnType::Bigint => "i64",
        ColumnType::Double => "f64",
        ColumnType::Boolean => "bool",
        ColumnType::Date => "::chrono::NaiveDate",
        ColumnType::Timestamp => "::chrono::DateTime<::chrono::Utc>",
        ColumnType::ListVarchar => "&[String]",
    };
    if param.nullable {
        format!("Option<{base}>")
    } else {
        base.to_string()
    }
}

/// Rewrite the statement's `$name` placeholders for the generated function: the
/// snapshot pin `$snapshot` becomes the `{snapshot}` splice marker, every other
/// parameter becomes a positional `$N` in `bound` order.
fn build_sql_template(sql: &str, snapshot: Option<&str>, bound: &[&StatementParam]) -> String {
    let mut out = sql.to_string();
    if let Some(snap) = snapshot {
        let re = Regex::new(&format!(r"\${}\b", regex::escape(snap))).unwrap();
        out = re.replace_all(&out, NoExpand("{snapshot}")).to_string();
    }
    for (i, param) in bound.iter().enumerate() {
        let re = Regex::new(&format!(r"\${}\b", regex::escape(&param.name))).unwrap();
        out = re
            .replace_all(&out, NoExpand(&format!("${}", i + 1)))
            .to_string();
    }
    out
}

/// Escape a SQL template for embedding in a Rust string literal.
fn rust_string_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Emit one lake read function returning `Vec<RecordBatch>`.
fn emit_lake_read_fn(stmt: &LoadedStatement) -> String {
    let fn_name = stmt
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("query");

    // The snapshot parameter (spliced) is identified by the AT-version rewrite;
    // every other declared parameter is bound.
    let snapshot = rewrite_lake(&stmt.sql).snapshot_param;
    let bound: Vec<&StatementParam> = stmt
        .params
        .iter()
        .filter(|p| Some(&p.name) != snapshot.as_ref())
        .collect();

    let template = build_sql_template(&stmt.sql, snapshot.as_deref(), &bound);
    let escaped = rust_string_escape(&template);

    // Arguments: the connection, then every declared parameter in yaml order.
    let mut args = String::from("    conn: &::duckdb::Connection,\n");
    for param in &stmt.params {
        args.push_str(&format!("    {}: {},\n", param.name, param_arg_type(param)));
    }

    let bind_list = bound
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>()
        .join(", ");

    let mut out = String::new();
    out.push_str(&format!(
        "    /// {}\n",
        stmt.description.replace(['\n', '\r'], " ").trim()
    ));
    out.push_str(&format!(
        "    pub fn {fn_name}(\n{args}    ) -> ::duckdb::Result<Vec<::arrow::record_batch::RecordBatch>> {{\n"
    ));
    out.push_str(&format!("        const SQL: &str = \"{escaped}\";\n"));
    out.push_str("        let sql = SQL.replace(\"{snapshot}\", &snapshot.to_string());\n");
    out.push_str("        let mut stmt = conn.prepare(&sql)?;\n");
    out.push_str(&format!(
        "        let batches = stmt.query_arrow(::duckdb::params![{bind_list}])?.collect::<Vec<_>>();\n"
    ));
    out.push_str("        Ok(batches)\n");
    out.push_str("    }\n");
    out
}

/// Emit the `pub mod <op> { .. }` query modules for an entity, grouping its
/// statements by operation. Returns `None` when the entity has no statements that
/// this phase emits (only lake reads are emitted today; writes land with the
/// control plane). The text is appended to the entity's generated Rust module.
#[must_use]
pub fn emit_query_modules(entity: &Entity, statements: &[LoadedStatement]) -> Option<String> {
    // Only lake reads are emitted in this phase.
    if !entity.has_store(Store::Duckdb) {
        return None;
    }
    let mut by_op: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for stmt in statements {
        if stmt.entity != entity.name {
            continue;
        }
        if stmt.operation == Operation::Read {
            by_op
                .entry(stmt.operation.folder())
                .or_default()
                .push(emit_lake_read_fn(stmt));
        }
    }
    if by_op.is_empty() {
        return None;
    }

    let mut out = String::new();
    for (op, fns) in by_op {
        out.push_str(&format!("\npub mod {op} {{\n"));
        out.push_str("    //! @generated query functions for this entity + operation.\n");
        for (i, f) in fns.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(f);
        }
        out.push_str("}\n");
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statement::load_statements;
    use er_entity_spec::Catalog;
    use std::path::PathBuf;

    fn entities_dir() -> PathBuf {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../entities"))
    }

    #[test]
    fn emits_golden_record_read_module() {
        let cat = Catalog::load_dir(entities_dir()).unwrap();
        let stmts = load_statements(&entities_dir(), &cat).unwrap();
        let entity = cat.by_name("golden_record").unwrap();
        let module = emit_query_modules(entity, &stmts).expect("golden_record has reads");

        assert!(module.contains("pub mod read {"));
        assert!(module.contains("pub fn list_page("));
        assert!(module.contains("pub fn by_id("));
        // The connection plus every declared parameter, typed.
        assert!(module.contains("conn: &::duckdb::Connection"));
        assert!(module.contains("snapshot: i64"));
        assert!(module.contains("after: Option<&str>"));
        assert!(module.contains("limit: i64"));
        assert!(module.contains("entity_id: &str"));
        // The snapshot is spliced; after/q/limit are bound $1/$2/$3 in order.
        assert!(module.contains("AT (VERSION => {snapshot})"));
        assert!(module.contains("entity_id > $1"));
        assert!(module.contains("params![after, q, limit]"));
        // ESCAPE backslash survives as a Rust-escaped literal.
        assert!(module.contains("ESCAPE '\\\\'"));
    }
}
