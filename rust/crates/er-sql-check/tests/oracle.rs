//! The oracle. DataFusion's coercions are not DuckDB's, so for every statement
//! this asks the real database what its result schema is — without running it —
//! and compares it to the entity. `DESCRIBE <statement>` types a query's output;
//! DuckDB cannot type an unbound placeholder, so each `$param` is substituted with
//! a typed null literal (`NULL::VARCHAR`) first. The snapshot pin is stripped with
//! the lake rewrite, so the described query runs against a plain attached catalog
//! named `lake` — no DuckLake extension needed.

use std::path::PathBuf;

use er_entity_spec::Catalog;
use er_sql_check::{load_statements, rewrite_lake};
use regex::{NoExpand, Regex};

fn entities_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../entities"))
}

/// Substitute every `$param` with a typed null literal of its DuckDB type, so
/// `DESCRIBE` can type the query. The substitution is textual and only for the
/// oracle, never for the generated function.
fn substitute_null_literals(sql: &str, stmt: &er_sql_check::LoadedStatement) -> String {
    let mut out = sql.to_string();
    for param in &stmt.params {
        let re = Regex::new(&format!(r"\${}\b", regex::escape(&param.name))).unwrap();
        let null = format!("NULL::{}", param.param_type.ducklake_type());
        out = re.replace_all(&out, NoExpand(&null)).to_string();
    }
    out
}

#[test]
fn oracle_golden_reads_match_duckdb_result_types() {
    let catalog = Catalog::load_dir(entities_dir()).expect("entities load");
    let entity = catalog
        .by_name("golden_record")
        .expect("golden_record entity");
    let statements = load_statements(&entities_dir(), &catalog).expect("statements load");

    let conn = duckdb::Connection::open_in_memory().expect("open duckdb");
    // JSON is DuckDB's built-in type (metadata); autoload where available, else the
    // extensions-enabled tier (DEDUPE_E2E_EXTENSIONS) has it cached.
    let _ = conn.execute_batch("INSTALL json; LOAD json;");
    // A plain in-memory catalog named `lake` with its default `main` schema, so the
    // fully-qualified `lake.main.golden_records` references resolve.
    conn.execute_batch("ATTACH ':memory:' AS lake;")
        .expect("attach lake");
    let ddl = er_entity_gen::emit_lake_ddl(entity).expect("lake DDL");
    conn.execute_batch(&ddl).expect("create golden_records");

    // What DuckDB should report for a `returns: entity` read: the entity's columns,
    // in order, by their DuckDB type.
    let expected: Vec<(String, String)> = entity
        .columns
        .iter()
        .map(|c| (c.name.clone(), c.col_type.ducklake_type().to_string()))
        .collect();

    for stmt in statements.iter().filter(|s| s.entity == "golden_record") {
        let rewritten = rewrite_lake(&stmt.sql);
        let described = substitute_null_literals(&rewritten.sql, stmt);
        let mut prepared = conn
            .prepare(&format!("DESCRIBE {described}"))
            .unwrap_or_else(|e| panic!("DESCRIBE failed for {:?}: {e}", stmt.path));
        let rows: Vec<(String, String)> = prepared
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .expect("describe rows")
            .collect::<duckdb::Result<Vec<_>>>()
            .expect("collect describe rows");
        assert_eq!(
            rows, expected,
            "oracle: DuckDB's result schema for {:?} does not equal the entity",
            stmt.path
        );
    }
}
