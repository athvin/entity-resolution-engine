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
use crate::statement::{LoadedStatement, Operation, ReturnColumn, Returns, StatementParam};

/// The Rust argument type for a declared parameter (references for strings).
fn param_arg_type(param: &StatementParam) -> String {
    let base = match param.param_type {
        ColumnType::Varchar => "&str",
        // A JSON parameter binds as jsonb (tokio_postgres) / JSON value, not text.
        ColumnType::Json => "&::serde_json::Value",
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

/// The shared skeleton of a lake read function: its name, argument list, bind
/// list, escaped SQL template, and the snapshot splice parameter (if the
/// statement pins one by name — a `pin: live` read or a literal pin has none).
struct LakeReadParts {
    fn_name: String,
    args: String,
    bind_list: String,
    escaped: String,
    /// The `$name` of the snapshot pin to splice, when the pin is a parameter.
    splice: Option<String>,
}

fn lake_read_parts(stmt: &LoadedStatement) -> LakeReadParts {
    let fn_name = stmt
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("query")
        .to_string();

    // The snapshot parameter (spliced) is identified by the AT-version rewrite;
    // a literal pin (`AT (VERSION => 42)`) is left verbatim in the SQL. Every
    // other declared parameter is bound.
    let splice = rewrite_lake(&stmt.sql)
        .snapshot_param
        .filter(|s| s.chars().next().is_some_and(char::is_alphabetic));
    let bound: Vec<&StatementParam> = stmt
        .params
        .iter()
        .filter(|p| Some(&p.name) != splice.as_ref())
        .collect();

    let template = build_sql_template(&stmt.sql, splice.as_deref(), &bound);
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

    LakeReadParts {
        fn_name,
        args,
        bind_list,
        escaped,
        splice,
    }
}

/// Emit the statement's prepare + `query_arrow` body lines: the snapshot splice
/// (when the pin is a parameter) and the positional binds.
fn push_lake_query_body(out: &mut String, parts: &LakeReadParts) {
    out.push_str(&format!(
        "        const SQL: &str = \"{}\";\n",
        parts.escaped
    ));
    if let Some(name) = &parts.splice {
        out.push_str(&format!(
            "        let sql = SQL.replace(\"{{snapshot}}\", &{name}.to_string());\n"
        ));
        out.push_str("        let mut stmt = conn.prepare(&sql)?;\n");
    } else {
        out.push_str("        let mut stmt = conn.prepare(SQL)?;\n");
    }
    out.push_str(&format!(
        "        let batches = stmt.query_arrow(::duckdb::params![{}])?.collect::<Vec<_>>();\n",
        parts.bind_list
    ));
}

/// Emit one `returns: entity` lake read function returning `Vec<RecordBatch>`
/// (the Arrow-wire path).
fn emit_lake_read_fn(stmt: &LoadedStatement) -> String {
    let parts = lake_read_parts(stmt);
    let mut out = String::new();
    out.push_str(&format!(
        "    /// {}\n",
        stmt.description.replace(['\n', '\r'], " ").trim()
    ));
    out.push_str(&format!(
        "    pub fn {}(\n{}    ) -> ::duckdb::Result<Vec<::arrow::record_batch::RecordBatch>> {{\n",
        parts.fn_name, parts.args
    ));
    push_lake_query_body(&mut out, &parts);
    out.push_str("        Ok(batches)\n");
    out.push_str("    }\n");
    out
}

/// The `::arrow::datatypes::DataType` constructor expression for a declared
/// returns column (fully qualified — the generated module has no `use`s). Mirrors
/// er-entity-gen's `arrow_data_type` map; the cross-test in er-entities'
/// `row_extract` holds the two together against a real DuckDB.
fn arrow_type_expr(col_type: ColumnType) -> &'static str {
    match col_type {
        // JSON crosses the Arrow boundary as text; `from_batches` parses it.
        ColumnType::Varchar | ColumnType::Json => "::arrow::datatypes::DataType::Utf8",
        ColumnType::Bigint => "::arrow::datatypes::DataType::Int64",
        ColumnType::Double => "::arrow::datatypes::DataType::Float64",
        ColumnType::Boolean => "::arrow::datatypes::DataType::Boolean",
        ColumnType::Date => "::arrow::datatypes::DataType::Date32",
        ColumnType::Timestamp => {
            "::arrow::datatypes::DataType::Timestamp(::arrow::datatypes::TimeUnit::Microsecond, None)"
        }
        ColumnType::ListVarchar => {
            "::arrow::datatypes::DataType::List(::std::sync::Arc::new(::arrow::datatypes::Field::new(\"item\", ::arrow::datatypes::DataType::Utf8, true)))"
        }
    }
}

/// The Arrow array type a declared returns column downcasts to.
fn arrow_array_type(col_type: ColumnType) -> &'static str {
    match col_type {
        ColumnType::Varchar | ColumnType::Json => "::arrow::array::StringArray",
        ColumnType::Bigint => "::arrow::array::Int64Array",
        ColumnType::Double => "::arrow::array::Float64Array",
        ColumnType::Boolean => "::arrow::array::BooleanArray",
        ColumnType::Date => "::arrow::array::Date32Array",
        ColumnType::Timestamp => "::arrow::array::TimestampMicrosecondArray",
        ColumnType::ListVarchar => "::arrow::array::ListArray",
    }
}

/// The Rust field type for a declared returns column (owned values, `Option` when
/// nullable), matching the entity-struct field map.
fn row_field_type(col: &ReturnColumn) -> String {
    let base = col.col_type.rust_type();
    if col.nullable {
        format!("Option<{base}>")
    } else {
        base.to_string()
    }
}

/// The per-row extraction expression for a declared returns column: the matching
/// `crate::batch` helper, wrapped in `req` when the column is non-null. `json` and
/// `list<varchar>` extraction is fallible on its own (malformed JSON, NULL list
/// element), hence the `?`.
fn row_extract_expr(col: &ReturnColumn, var: &str) -> String {
    let name = &col.name;
    let opt = match col.col_type {
        ColumnType::Varchar => format!("crate::batch::opt_str({var}, i)"),
        ColumnType::Bigint => format!("crate::batch::opt_i64({var}, i)"),
        ColumnType::Double => format!("crate::batch::opt_f64({var}, i)"),
        ColumnType::Boolean => format!("crate::batch::opt_bool({var}, i)"),
        ColumnType::Date => format!("crate::batch::opt_date({var}, i)"),
        ColumnType::Timestamp => format!("crate::batch::opt_ts({var}, i)"),
        ColumnType::Json => format!("crate::batch::opt_json({var}, i, \"{name}\")?"),
        ColumnType::ListVarchar => format!("crate::batch::opt_list_str({var}, i, \"{name}\")?"),
    };
    if col.nullable {
        opt
    } else {
        format!("crate::batch::req({opt}, \"{name}\", i)?")
    }
}

/// Emit one `returns: columns` lake read: the bespoke typed row struct (declared
/// result schema compiled in, `from_batches` conversion that errors — never
/// panics — on any shape/type/null violation) and the query function returning
/// `Vec<Row>`.
fn emit_lake_read_rows_fn(stmt: &LoadedStatement, cols: &[ReturnColumn]) -> String {
    let parts = lake_read_parts(stmt);
    let struct_name = format!("{}Row", to_pascal_case(&parts.fn_name));
    let description = stmt.description.replace(['\n', '\r'], " ");
    let description = description.trim();

    let mut out = String::new();

    // The row struct.
    out.push_str(&format!(
        "    /// One row of `{}`: {description}\n",
        parts.fn_name
    ));
    out.push_str(
        "    #[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::utoipa::ToSchema)]\n",
    );
    out.push_str(&format!("    pub struct {struct_name} {{\n"));
    for col in cols {
        out.push_str(&format!(
            "        pub {}: {},\n",
            col.name,
            row_field_type(col)
        ));
    }
    out.push_str("    }\n\n");

    // expected_fields + from_batches.
    out.push_str(&format!("    impl {struct_name} {{\n"));
    out.push_str("        /// The declared result schema, the compiled-in expected shape.\n");
    out.push_str("        #[must_use]\n");
    out.push_str(
        "        pub fn expected_fields() -> Vec<(&'static str, ::arrow::datatypes::DataType)> {\n",
    );
    out.push_str("            vec![\n");
    for col in cols {
        out.push_str(&format!(
            "                (\"{}\", {}),\n",
            col.name,
            arrow_type_expr(col.col_type)
        ));
    }
    out.push_str("            ]\n        }\n\n");

    out.push_str("        /// Convert Arrow batches to typed rows; any shape, type or null\n");
    out.push_str("        /// violation is an `Err`, never a panic.\n");
    out.push_str(
        "        pub fn from_batches(\n            batches: &[::arrow::record_batch::RecordBatch],\n        ) -> ::duckdb::Result<Vec<Self>> {\n",
    );
    out.push_str("            let expected = Self::expected_fields();\n");
    out.push_str("            let mut out = Vec::new();\n");
    out.push_str("            for batch in batches {\n");
    out.push_str("                crate::batch::check_columns(batch, &expected)?;\n");
    for (i, col) in cols.iter().enumerate() {
        out.push_str(&format!(
            "                let c{i}: &{} = crate::batch::col(batch, {i}, \"{}\")?;\n",
            arrow_array_type(col.col_type),
            col.name
        ));
    }
    out.push_str("                for i in 0..batch.num_rows() {\n");
    out.push_str("                    out.push(Self {\n");
    for (i, col) in cols.iter().enumerate() {
        out.push_str(&format!(
            "                        {}: {},\n",
            col.name,
            row_extract_expr(col, &format!("c{i}"))
        ));
    }
    out.push_str("                    });\n");
    out.push_str("                }\n");
    out.push_str("            }\n");
    out.push_str("            Ok(out)\n");
    out.push_str("        }\n");
    out.push_str("    }\n\n");

    // The query function.
    out.push_str(&format!("    /// {description}\n"));
    out.push_str(&format!(
        "    pub fn {}(\n{}    ) -> ::duckdb::Result<Vec<{struct_name}>> {{\n",
        parts.fn_name, parts.args
    ));
    push_lake_query_body(&mut out, &parts);
    out.push_str(&format!("        {struct_name}::from_batches(&batches)\n"));
    out.push_str("    }\n");
    out
}

/// Emit one lake write function returning the affected-row count. No snapshot pin
/// (E006 is a read rule); every declared parameter binds positionally. Timestamps
/// and JSON cross as text with an explicit `::timestamp` / `::json` cast in the
/// statement SQL, matching the engine's string-typed logical timestamps.
fn emit_lake_write_fn(stmt: &LoadedStatement) -> String {
    let fn_name = stmt
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("query");
    let bound: Vec<&StatementParam> = stmt.params.iter().collect();
    let template = build_sql_template(&stmt.sql, None, &bound);
    let escaped = rust_string_escape(&template);

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
        "    pub fn {fn_name}(\n{args}    ) -> ::duckdb::Result<usize> {{\n"
    ));
    out.push_str(&format!("        const SQL: &str = \"{escaped}\";\n"));
    out.push_str(&format!(
        "        conn.execute(SQL, ::duckdb::params![{bind_list}])\n"
    ));
    out.push_str("    }\n");
    out
}

/// `snake_case` -> `PascalCase`, matching the generated struct name in er-entity-gen.
fn to_pascal_case(name: &str) -> String {
    name.split(['_', '-'])
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Emit one control-plane function. A statement that returns the entity (a read,
/// or a write with `RETURNING`) runs `client.query` and maps each
/// `tokio_postgres::Row` into the entity struct by column name, returning
/// `Vec<Entity>`. A `returns: none` write runs `client.execute` and returns the
/// affected-row count. Every declared parameter is bound positionally as `$1`,
/// `$2`, … in yaml order; the real SQL (incl. RETURNING / ON CONFLICT / FOR
/// UPDATE) is run verbatim.
fn emit_control_fn(entity: &Entity, stmt: &LoadedStatement) -> String {
    let fn_name = stmt
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("query");
    let struct_name = to_pascal_case(&entity.name);
    let bound: Vec<&StatementParam> = stmt.params.iter().collect();
    let template = build_sql_template(&stmt.sql, None, &bound);
    let escaped = rust_string_escape(&template);

    let mut args = String::from("    client: &::tokio_postgres::Client,\n");
    for param in &stmt.params {
        args.push_str(&format!("    {}: {},\n", param.name, param_arg_type(param)));
    }
    let bind_list = bound
        .iter()
        .map(|p| format!("&{}", p.name))
        .collect::<Vec<_>>()
        .join(", ");

    let mut out = String::new();
    out.push_str(&format!(
        "    /// {}\n",
        stmt.description.replace(['\n', '\r'], " ").trim()
    ));

    if stmt.returns == Returns::None {
        // A write with no result rows: return the affected-row count.
        out.push_str(&format!(
            "    pub async fn {fn_name}(\n{args}    ) -> ::std::result::Result<u64, ::tokio_postgres::Error> {{\n"
        ));
        out.push_str(&format!("        const SQL: &str = \"{escaped}\";\n"));
        out.push_str(&format!(
            "        client.execute(SQL, &[{bind_list}]).await\n"
        ));
        out.push_str("    }\n");
        return out;
    }

    let field_map = entity
        .columns
        .iter()
        .map(|c| format!("                {}: row.get(\"{}\"),", c.name, c.name))
        .collect::<Vec<_>>()
        .join("\n");
    out.push_str(&format!(
        "    pub async fn {fn_name}(\n{args}    ) -> ::std::result::Result<Vec<super::{struct_name}>, ::tokio_postgres::Error> {{\n"
    ));
    out.push_str(&format!("        const SQL: &str = \"{escaped}\";\n"));
    out.push_str(&format!(
        "        let rows = client.query(SQL, &[{bind_list}]).await?;\n"
    ));
    out.push_str("        Ok(rows\n            .iter()\n            .map(|row| super::");
    out.push_str(&format!(
        "{struct_name} {{\n{field_map}\n            }})\n            .collect())\n"
    ));
    out.push_str("    }\n");
    out
}

/// Emit the `pub mod <op> { .. }` query modules for an entity, grouping its read
/// statements by operation: lake reads return Arrow `RecordBatch` via DuckDB,
/// control reads map `tokio_postgres::Row` into the entity struct. Writes land
/// with the control-plane rewrite (3c). Returns `None` when the entity has no
/// emitted statements. The text is appended to the entity's generated module.
#[must_use]
pub fn emit_query_modules(entity: &Entity, statements: &[LoadedStatement]) -> Option<String> {
    let mut by_op: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for stmt in statements {
        if stmt.entity != entity.name {
            continue;
        }
        let f = if entity.has_store(Store::Duckdb) {
            // Lake: a `returns: entity` read returns Arrow batches (the Arrow-wire
            // path); a `returns: columns` read returns the bespoke typed rows;
            // writes (INSERT/UPDATE/DELETE/MERGE under their folders, `returns:
            // none`) return the affected-row count. A lake write that claims to
            // return rows has no emitter — loudly unsupported rather than
            // silently skipped.
            match (stmt.operation, &stmt.returns) {
                (Operation::Read, Returns::Columns(cols)) => emit_lake_read_rows_fn(stmt, cols),
                (Operation::Read, Returns::Entity) => emit_lake_read_fn(stmt),
                (Operation::Read, Returns::None) => {
                    unreachable!("rejected at load: a read cannot return none")
                }
                (_, Returns::None) => emit_lake_write_fn(stmt),
                _ => panic!(
                    "{}: a lake write must declare `returns: none` (RETURNING is not emitted for the lake)",
                    stmt.path.display()
                ),
            }
        } else if entity.has_store(Store::Postgres) {
            // Control: reads and writes (create/update/delete), including
            // RETURNING. `returns: columns` is lake-read-only in this phase (the
            // E012 diagnostic says so first; this is the backstop).
            assert!(
                !matches!(stmt.returns, Returns::Columns(_)),
                "{}: returns: columns is lake-read-only in this phase; a control read declares `returns: entity`",
                stmt.path.display()
            );
            emit_control_fn(entity, stmt)
        } else {
            continue;
        };
        by_op.entry(stmt.operation.folder()).or_default().push(f);
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
    fn emits_job_control_read_module() {
        let cat = Catalog::load_dir(entities_dir()).unwrap();
        let stmts = load_statements(&entities_dir(), &cat).unwrap();
        let entity = cat.by_name("job").unwrap();
        let module = emit_query_modules(entity, &stmts).expect("job has reads");

        assert!(module.contains("pub mod read {"));
        assert!(module.contains("pub async fn by_id("));
        assert!(module.contains("pub async fn list("));
        assert!(module.contains("client: &::tokio_postgres::Client"));
        assert!(
            module.contains("-> ::std::result::Result<Vec<super::Job>, ::tokio_postgres::Error>")
        );
        // Rows map into the struct by column name; params bind positionally.
        assert!(module.contains("job_id: row.get(\"job_id\"),"));
        assert!(module.contains("WHERE org = $1 AND job_id = $2"));
        assert!(module.contains("client.query(SQL, &[&org, &job_id]).await?"));
    }

    #[test]
    fn emits_job_control_write_modules() {
        let cat = Catalog::load_dir(entities_dir()).unwrap();
        let stmts = load_statements(&entities_dir(), &cat).unwrap();
        let entity = cat.by_name("job").unwrap();
        let module = emit_query_modules(entity, &stmts).expect("job has statements");

        // create: enqueue returns the entity via RETURNING (query), jsonb param typed.
        assert!(module.contains("pub mod create {"));
        assert!(module.contains("pub async fn enqueue("));
        assert!(module.contains("params: Option<&::serde_json::Value>"));
        assert!(module.contains("ON CONFLICT"));
        // update: claim returns the entity; mark returns none -> execute -> u64.
        assert!(module.contains("pub mod update {"));
        assert!(module.contains("pub async fn claim("));
        assert!(module.contains("pub async fn mark("));
        assert!(module.contains("-> ::std::result::Result<u64, ::tokio_postgres::Error>"));
        assert!(module.contains("client.execute(SQL,"));
    }

    #[test]
    fn emits_run_lake_write_module() {
        let cat = Catalog::load_dir(entities_dir()).unwrap();
        let stmts = load_statements(&entities_dir(), &cat).unwrap();
        let entity = cat.by_name("run").unwrap();
        let module = emit_query_modules(entity, &stmts).expect("run has writes");

        // A lake write binds positionally on the DuckDB connection and returns
        // the affected-row count; no snapshot splice on a write.
        assert!(module.contains("pub mod create {"));
        assert!(module.contains("pub fn start("));
        assert!(module.contains("conn: &::duckdb::Connection"));
        assert!(module.contains("-> ::duckdb::Result<usize>"));
        assert!(module.contains("conn.execute(SQL, ::duckdb::params!["));
        assert!(module.contains("VALUES ($1, $2, $3, 'running', $4::timestamp"));
        assert!(!module.contains("{snapshot}"));
        assert!(module.contains("pub mod update {"));
        assert!(module.contains("pub fn finish("));
    }

    fn columns_stmt(
        sql: &str,
        params: Vec<StatementParam>,
        cols: Vec<ReturnColumn>,
    ) -> LoadedStatement {
        LoadedStatement {
            entity: "entity_membership".to_string(),
            operation: Operation::Read,
            description: "test".to_string(),
            params,
            returns: Returns::Columns(cols),
            mode: crate::Mode::Full,
            reads: vec![],
            pin: crate::Pin::Snapshot,
            pin_reason: String::new(),
            sql: sql.to_string(),
            path: std::path::PathBuf::from("entities/entity_membership/read/test.sql"),
        }
    }

    fn varchar_col(name: &str, nullable: bool) -> ReturnColumn {
        ReturnColumn {
            name: name.to_string(),
            col_type: ColumnType::Varchar,
            nullable,
        }
    }

    #[test]
    fn emits_member_keys_rows_module() {
        let cat = Catalog::load_dir(entities_dir()).unwrap();
        let stmts = load_statements(&entities_dir(), &cat).unwrap();
        let entity = cat.by_name("entity_membership").unwrap();
        let module = emit_query_modules(entity, &stmts).expect("entity_membership has statements");

        // The bespoke row struct with the declared non-null field, serde + utoipa.
        assert!(module.contains("pub struct MemberKeysRow {"));
        assert!(module.contains("pub record_key: String,"));
        assert!(module.contains("::serde::Serialize, ::utoipa::ToSchema"));
        // The compiled-in expected shape and the fallible conversion.
        assert!(module.contains("pub fn expected_fields()"));
        assert!(module.contains("(\"record_key\", ::arrow::datatypes::DataType::Utf8)"));
        assert!(module.contains("crate::batch::check_columns(batch, &expected)?;"));
        assert!(
            module.contains("crate::batch::req(crate::batch::opt_str(c0, i), \"record_key\", i)?")
        );
        // The query fn returns typed rows; the snapshot splices, entity_id binds.
        assert!(module.contains("-> ::duckdb::Result<Vec<MemberKeysRow>>"));
        assert!(module.contains("let sql = SQL.replace(\"{snapshot}\", &snapshot.to_string());"));
        assert!(module.contains("params![entity_id]"));
    }

    #[test]
    fn a_live_read_emits_no_snapshot_splice() {
        let stmt = columns_stmt(
            "SELECT record_key FROM lake.main.entity_membership WHERE entity_id = $entity_id",
            vec![StatementParam {
                name: "entity_id".to_string(),
                param_type: ColumnType::Varchar,
                nullable: false,
            }],
            vec![varchar_col("record_key", false)],
        );
        let f = emit_lake_read_rows_fn(&stmt, &[varchar_col("record_key", false)]);
        assert!(!f.contains("{snapshot}"), "no splice marker: {f}");
        assert!(
            f.contains("let mut stmt = conn.prepare(SQL)?;"),
            "prepares the const: {f}"
        );
        assert!(f.contains("params![entity_id]"));
    }

    #[test]
    fn a_custom_pin_name_is_spliced_by_its_own_name() {
        let stmt = columns_stmt(
            "SELECT record_key FROM lake.main.entity_membership AT (VERSION => $snap)",
            vec![StatementParam {
                name: "snap".to_string(),
                param_type: ColumnType::Bigint,
                nullable: false,
            }],
            vec![varchar_col("record_key", true)],
        );
        let f = emit_lake_read_rows_fn(&stmt, &[varchar_col("record_key", true)]);
        assert!(
            f.contains("let sql = SQL.replace(\"{snapshot}\", &snap.to_string());"),
            "the splice references the pin's own name: {f}"
        );
        // A nullable column stays Option and skips `req`.
        assert!(f.contains("pub record_key: Option<String>,"));
        assert!(f.contains("record_key: crate::batch::opt_str(c0, i),"));
    }

    #[test]
    fn a_literal_pin_emits_valid_code_with_the_pin_verbatim() {
        let stmt = columns_stmt(
            "SELECT record_key FROM lake.main.entity_membership AT (VERSION => 42)",
            vec![],
            vec![varchar_col("record_key", true)],
        );
        let f = emit_lake_read_rows_fn(&stmt, &[varchar_col("record_key", true)]);
        assert!(
            f.contains("AT (VERSION => 42)"),
            "literal pin survives: {f}"
        );
        assert!(!f.contains("{snapshot}"));
        assert!(f.contains("let mut stmt = conn.prepare(SQL)?;"));
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
