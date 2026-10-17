//! The oracle. DataFusion's coercions are not DuckDB's, so for every lake read
//! statement this asks the real database what its result schema is — without
//! running it — and compares it to the declaration. `DESCRIBE <statement>` types
//! a query's output; DuckDB cannot type an unbound placeholder, so each `$param`
//! is substituted with a typed null literal (`NULL::VARCHAR`) first. The snapshot
//! pin is stripped with the lake rewrite, so the described query runs against a
//! plain attached catalog named `lake` — no DuckLake extension needed. This is
//! the check that catches DuckDB-vs-DataFusion divergence the analyser cannot
//! (e.g. `sum(BIGINT)` typing as HUGEINT), and the only shape check a
//! `mode: partial` read gets.

use std::path::PathBuf;

use er_entity_spec::{Catalog, Store};
use er_sql_check::{Operation, Returns, load_statements, rewrite_lake};
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
fn oracle_lake_reads_match_their_declared_shape() {
    let catalog = Catalog::load_dir(entities_dir()).expect("entities load");
    let statements = load_statements(&entities_dir(), &catalog).expect("statements load");

    let conn = duckdb::Connection::open_in_memory().expect("open duckdb");
    // JSON is DuckDB's built-in type (metadata); autoload where available, else the
    // extensions-enabled tier (DEDUPE_E2E_EXTENSIONS) has it cached.
    let _ = conn.execute_batch("INSTALL json; LOAD json;");
    // A plain in-memory catalog named `lake` with its default `main` schema, so the
    // fully-qualified `lake.main.<relation>` references resolve. Every lake-store
    // entity's table is created from the generated DDL, so a statement may read
    // across entities.
    conn.execute_batch("ATTACH ':memory:' AS lake;")
        .expect("attach lake");
    for entity in catalog.for_store(Store::Duckdb) {
        let ddl = er_entity_gen::emit_lake_ddl(entity).expect("lake DDL");
        conn.execute_batch(&ddl)
            .unwrap_or_else(|e| panic!("create {}: {e}", entity.relation));
    }

    let mut described = 0usize;
    for stmt in statements.iter().filter(|s| s.operation == Operation::Read) {
        let entity = catalog.by_name(&stmt.entity).expect("statement entity");
        if !entity.has_store(Store::Duckdb) {
            continue;
        }

        // What DuckDB should report: the entity's columns for `returns: entity`,
        // the declared typed list for `returns: columns`, each by DuckDB type.
        let expected: Vec<(String, String)> = match &stmt.returns {
            Returns::Entity => entity
                .columns
                .iter()
                .map(|c| (c.name.clone(), c.col_type.ducklake_type().to_string()))
                .collect(),
            Returns::Columns(cols) => cols
                .iter()
                .map(|c| (c.name.clone(), c.col_type.ducklake_type().to_string()))
                .collect(),
            Returns::None => unreachable!("a read cannot return none (rejected at load)"),
        };

        let rewritten = rewrite_lake(&stmt.sql);
        let described_sql = substitute_null_literals(&rewritten.sql, stmt);
        let mut prepared = conn
            .prepare(&format!("DESCRIBE {described_sql}"))
            .unwrap_or_else(|e| panic!("DESCRIBE failed for {:?}: {e}", stmt.path));
        let rows: Vec<(String, String)> = prepared
            .query_map([], |row| {
                let name = row.get::<_, String>(0)?;
                let mut col_type = row.get::<_, String>(1)?;
                // DuckDB reports a list as `VARCHAR[]`; the vocabulary's logical
                // spelling is `LIST(VARCHAR)` (same normalization er-entity-diff
                // applies).
                if col_type == "VARCHAR[]" {
                    col_type = "LIST(VARCHAR)".to_string();
                }
                Ok((name, col_type))
            })
            .expect("describe rows")
            .collect::<duckdb::Result<Vec<_>>>()
            .expect("collect describe rows");
        assert_eq!(
            rows, expected,
            "oracle: DuckDB's result schema for {:?} does not equal the declaration",
            stmt.path
        );
        described += 1;
    }
    // The oracle must actually cover the tree: at least the golden list/by_id
    // pairs and the first columns read.
    assert!(described >= 5, "only {described} lake reads described");
}
