//! The entity passes. Each loaded statement goes through the lake rewrite, is
//! planned against the store catalog, and then checked by the E0xx rules. A
//! failing check is a [`Diagnostic`] naming its code, the statement path and the
//! cause. Type-level checks that need a real database (the oracle) land later;
//! these are the static rules that need only the plan and the entity.

use std::path::PathBuf;

use datafusion_common::arrow::datatypes::DataType;
use er_entity_spec::{Catalog, ColumnType, Entity, Store};
use ff_analysis::sqlparser::ast::{Query, Select, SelectItem, SetExpr, Statement, TableFactor};
use ff_analysis::sqlparser::dialect::{Dialect, DuckDbDialect, PostgreSqlDialect};
use ff_analysis::sqlparser::parser::Parser;
use ff_analysis::{FeatherFlowProvider, FunctionRegistry};

use crate::catalog::build_store_catalog;
use crate::rewrite::rewrite_lake;
use crate::statement::{
    LoadedStatement, Mode, Operation, Pin, Returns, StmtError, load_statements,
};

/// One analysis finding: a stable `E0xx` code, the offending statement, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// The rule code (`E001`..`E013`).
    pub code: &'static str,
    /// The `.sql` path the finding is about.
    pub path: PathBuf,
    /// A one-line explanation.
    pub message: String,
}

impl Diagnostic {
    fn new(code: &'static str, path: &std::path::Path, message: impl Into<String>) -> Self {
        Self {
            code,
            path: path.to_path_buf(),
            message: message.into(),
        }
    }
}

/// The store a statement is analysed against (its entity's store). A statement
/// belongs to exactly one store.
fn dialect_for(store: Store) -> Box<dyn Dialect> {
    match store {
        Store::Duckdb => Box::new(DuckDbDialect {}),
        Store::Postgres => Box::new(PostgreSqlDialect {}),
    }
}

/// Analyse one statement against its entity and the store provider, returning every
/// diagnostic it raises (empty when it passes).
#[must_use]
pub fn analyze_statement(
    stmt: &LoadedStatement,
    entity: &Entity,
    store: Store,
    provider: &FeatherFlowProvider,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    // Rewrite: strip the lake snapshot pin, recording whether one was present.
    let rewritten = rewrite_lake(&stmt.sql);

    // E006 — a lake read must be snapshot-pinned, unless it declares (and
    // justifies) `pin: live`; a live read must then NOT carry a pin, and `pin:
    // live` means nothing anywhere but a lake read.
    if store == Store::Duckdb && stmt.operation == Operation::Read {
        match (stmt.pin, rewritten.snapshot_param.is_some()) {
            (Pin::Snapshot, false) => diags.push(Diagnostic::new(
                "E006",
                &stmt.path,
                "a lake read must carry AT (VERSION => $snapshot); the read API's MVCC rule",
            )),
            (Pin::Live, true) => diags.push(Diagnostic::new(
                "E006",
                &stmt.path,
                "a `pin: live` read must not carry AT (VERSION => ..); drop the clause or drop `pin: live`",
            )),
            _ => {}
        }
    } else if stmt.pin == Pin::Live {
        diags.push(Diagnostic::new(
            "E006",
            &stmt.path,
            "`pin: live` is a lake-read concept; nothing else has a snapshot to opt out of",
        ));
    }

    // E002 — a list<varchar> parameter cannot bind on the lake (duckdb-rs has no
    // LIST binding); the lake idiom is a chr(31)-delimited varchar split in SQL.
    if store == Store::Duckdb {
        for param in &stmt.params {
            if param.param_type == ColumnType::ListVarchar {
                diags.push(Diagnostic::new(
                    "E002",
                    &stmt.path,
                    format!(
                        "parameter ${} is list<varchar>, which cannot bind on the lake \
                         (duckdb-rs has no LIST binding); pass a chr(31)-delimited varchar \
                         and split with string_split(${}, chr(31))",
                        param.name, param.name
                    ),
                ));
            }
        }
    }

    // E012 — `returns: columns` is lake-read-only in this phase; a control read
    // declares `returns: entity` (the control emitter maps rows by entity column).
    if store == Store::Postgres && matches!(stmt.returns, Returns::Columns(_)) {
        diags.push(Diagnostic::new(
            "E012",
            &stmt.path,
            "returns: columns is lake-read-only in this phase; a control read declares `returns: entity`",
        ));
    }

    // Partial mode: the visible exception DataFusion's parser cannot even read (a
    // write with ON CONFLICT ... WHERE, FOR UPDATE SKIP LOCKED, and the like).
    // Checked by text — the SQL's leading verb against its folder (E012), no
    // `SELECT *` (E011), and the declared parameters cover the `$placeholders`
    // (E002). The oracle supplies the output schema and the E004/E005 type checks.
    if stmt.mode == Mode::Partial {
        if let Some(code) = leading_verb_mismatch_text(&stmt.sql, stmt.operation) {
            diags.push(Diagnostic::new("E012", &stmt.path, code));
        }
        if regex::Regex::new(r"(?i)SELECT\s+\*")
            .unwrap()
            .is_match(&stmt.sql)
        {
            diags.push(Diagnostic::new(
                "E011",
                &stmt.path,
                "SELECT * is forbidden; name every column",
            ));
        }
        let declared: std::collections::BTreeSet<&str> =
            stmt.params.iter().map(|p| p.name.as_str()).collect();
        let placeholder = regex::Regex::new(r"\$([A-Za-z_]\w*)").unwrap();
        for caps in placeholder.captures_iter(&stmt.sql) {
            let name = &caps[1];
            if !declared.contains(name) {
                diags.push(Diagnostic::new(
                    "E002",
                    &stmt.path,
                    format!("statement uses ${name} but the yaml does not declare it"),
                ));
            }
        }
        return diags;
    }

    // Parse once for the structural checks, then plan.
    let dialect = dialect_for(store);
    let parsed = match Parser::parse_sql(dialect.as_ref(), &rewritten.sql) {
        Ok(mut stmts) if !stmts.is_empty() => stmts.remove(0),
        Ok(_) => {
            diags.push(Diagnostic::new("E001", &stmt.path, "no statement parsed"));
            return diags;
        }
        Err(e) => {
            diags.push(Diagnostic::new(
                "E001",
                &stmt.path,
                format!("parse error: {e}"),
            ));
            return diags;
        }
    };

    // E011 — no SELECT * anywhere.
    if statement_has_wildcard(&parsed) {
        diags.push(Diagnostic::new(
            "E011",
            &stmt.path,
            "SELECT * is forbidden; name every column so an added column cannot change the shape",
        ));
    }

    // E012 — the SQL's leading verb matches the folder/operation.
    if let Some(code) = verb_mismatch(&parsed, stmt.operation) {
        diags.push(Diagnostic::new("E012", &stmt.path, code));
    }

    // Plan, then the plan-dependent checks.
    let plan = match ff_analysis::plan_statement(parsed, provider) {
        Ok(plan) => plan,
        Err(e) => {
            diags.push(Diagnostic::new(
                "E001",
                &stmt.path,
                format!("plan error: {e}"),
            ));
            return diags;
        }
    };

    // E002 — every placeholder the planner inferred is a declared parameter, and
    // the stripped snapshot parameter is declared too.
    let declared: std::collections::BTreeSet<String> =
        stmt.params.iter().map(|p| p.name.clone()).collect();
    if let Ok(param_types) = plan.get_parameter_types() {
        for raw in param_types.keys() {
            let name = raw.trim_start_matches('$');
            if !declared.contains(name) {
                diags.push(Diagnostic::new(
                    "E002",
                    &stmt.path,
                    format!("statement uses ${name} but the yaml does not declare it"),
                ));
            }
        }
    }
    if let Some(snap) = &rewritten.snapshot_param {
        // A literal pin (e.g. `=> 42`) is not a parameter; only a `$name` pin is.
        if snap.chars().next().is_some_and(char::is_alphabetic) && !declared.contains(snap) {
            diags.push(Diagnostic::new(
                "E002",
                &stmt.path,
                format!("the snapshot pin ${snap} is not declared in the yaml"),
            ));
        }
    }

    // E003 — the statement's output equals its declaration. For `returns: entity`
    // that is the entity's columns, in order (type fidelity is the oracle's job).
    // For `returns: columns` it is the declared typed list: names in order, each
    // plan type compatible with the declared type, and a declared non-null column
    // must be provably non-null in the plan.
    match &stmt.returns {
        Returns::Entity => {
            let actual: Vec<String> = plan
                .schema()
                .fields()
                .iter()
                .map(|f| f.name().clone())
                .collect();
            let expected: Vec<String> = entity.columns.iter().map(|c| c.name.clone()).collect();
            if actual != expected {
                diags.push(Diagnostic::new(
                    "E003",
                    &stmt.path,
                    format!(
                        "output columns do not equal entity {}: expected {expected:?}, got {actual:?}",
                        entity.name
                    ),
                ));
            }
        }
        Returns::Columns(cols) => {
            let fields = plan.schema().fields();
            let actual: Vec<String> = fields.iter().map(|f| f.name().clone()).collect();
            let expected: Vec<String> = cols.iter().map(|c| c.name.clone()).collect();
            if actual == expected {
                for (col, field) in cols.iter().zip(fields.iter()) {
                    if !arrow_compatible(col.col_type, field.data_type()) {
                        diags.push(Diagnostic::new(
                            "E003",
                            &stmt.path,
                            format!(
                                "column {}: declared {} but the plan output is {:?}",
                                col.name,
                                col.col_type.ducklake_type(),
                                field.data_type()
                            ),
                        ));
                    }
                    if !col.nullable && field.is_nullable() {
                        diags.push(Diagnostic::new(
                            "E003",
                            &stmt.path,
                            format!(
                                "column {} is declared non-null but the plan cannot prove it \
                                 (an outer join or expression on its path); declare it nullable",
                                col.name
                            ),
                        ));
                    }
                }
            } else {
                diags.push(Diagnostic::new(
                    "E003",
                    &stmt.path,
                    format!(
                        "output columns do not equal the declared returns: expected {expected:?}, got {actual:?}"
                    ),
                ));
            }
        }
        Returns::None => {}
    }

    diags
}

/// Whether a declared column type is compatible with the plan's (arrow 57) output
/// type. DataFusion's coercions widen strings and pick its own timestamp unit, so
/// the relation is deliberately tolerant there; the oracle holds the exact line
/// against the real database.
fn arrow_compatible(declared: ColumnType, actual: &DataType) -> bool {
    match declared {
        // JSON plans as text (the `::json` casts are rewritten to `::varchar`).
        ColumnType::Varchar | ColumnType::Json => matches!(
            actual,
            DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
        ),
        ColumnType::Bigint => matches!(actual, DataType::Int64),
        ColumnType::Double => matches!(actual, DataType::Float64),
        ColumnType::Boolean => matches!(actual, DataType::Boolean),
        ColumnType::Date => matches!(actual, DataType::Date32),
        // Unit-insensitive: DuckDB decides microseconds at runtime; the oracle and
        // the generated `from_batches` hold that line.
        ColumnType::Timestamp => matches!(actual, DataType::Timestamp(_, None)),
        ColumnType::ListVarchar => match actual {
            DataType::List(item) | DataType::LargeList(item) => matches!(
                item.data_type(),
                DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
            ),
            _ => false,
        },
    }
}

/// Load and analyse every statement in the catalog, against each statement's
/// entity and store. Returns all diagnostics across the tree.
pub fn analyze_catalog(
    entities_dir: &std::path::Path,
    catalog: &Catalog,
) -> Result<Vec<Diagnostic>, StmtError> {
    let statements = load_statements(entities_dir, catalog)?;
    // One provider per store, reused across its statements.
    let lake_cat = build_store_catalog(catalog, Store::Duckdb);
    let pg_cat = build_store_catalog(catalog, Store::Postgres);
    let registry = FunctionRegistry::new();
    let lake_provider = FeatherFlowProvider::new(&lake_cat, &registry);
    let pg_provider = FeatherFlowProvider::new(&pg_cat, &registry);

    let mut diags = Vec::new();
    for stmt in &statements {
        let Some(entity) = catalog.by_name(&stmt.entity) else {
            continue;
        };
        // golden_record is a lake entity; the store is the entity's single store.
        let store = if entity.has_store(Store::Duckdb) {
            Store::Duckdb
        } else {
            Store::Postgres
        };
        let provider = if store == Store::Duckdb {
            &lake_provider
        } else {
            &pg_provider
        };
        diags.extend(analyze_statement(stmt, entity, store, provider));
    }
    Ok(diags)
}

// ── structural helpers over the sqlparser AST ──────────────────────────────

/// Text-based leading-verb check for partial statements (which do not parse): the
/// first SQL keyword must match the folder. Returns a message on mismatch.
fn leading_verb_mismatch_text(sql: &str, op: Operation) -> Option<String> {
    let first = sql
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    let ok = match op {
        Operation::Read => first == "SELECT" || first == "WITH" || first == "COPY",
        Operation::Create => first == "INSERT" || first == "COPY" || first == "MERGE",
        Operation::Update => first == "UPDATE" || first == "MERGE",
        Operation::Delete => first == "DELETE",
    };
    if ok {
        None
    } else {
        Some(format!(
            "statement verb {first:?} does not match its {:?} folder",
            op.folder()
        ))
    }
}

/// The leading verb does not match the folder. Returns an explanatory message
/// when it mismatches, `None` when it is fine.
fn verb_mismatch(stmt: &Statement, op: Operation) -> Option<String> {
    let ok = match op {
        // COPY ... TO is also a read; it is a `Statement::Copy`, handled with exports.
        Operation::Read => matches!(stmt, Statement::Query(_)),
        Operation::Create => {
            matches!(stmt, Statement::Insert(_))
        }
        Operation::Update => matches!(stmt, Statement::Update { .. }),
        Operation::Delete => matches!(stmt, Statement::Delete(_)),
    };
    if ok {
        None
    } else {
        Some(format!(
            "statement kind does not match its {:?} folder",
            op.folder()
        ))
    }
}

/// Whether a `SELECT *` (or `t.*`) appears anywhere the walk reaches: the final
/// projection, set-operation arms, derived (subquery) tables, and CTEs.
fn statement_has_wildcard(stmt: &Statement) -> bool {
    match stmt {
        Statement::Query(q) => query_has_wildcard(q),
        _ => false,
    }
}

fn query_has_wildcard(query: &Query) -> bool {
    if let Some(with) = &query.with {
        if with
            .cte_tables
            .iter()
            .any(|cte| query_has_wildcard(&cte.query))
        {
            return true;
        }
    }
    set_expr_has_wildcard(&query.body)
}

fn set_expr_has_wildcard(body: &SetExpr) -> bool {
    match body {
        SetExpr::Select(select) => select_has_wildcard(select),
        SetExpr::Query(q) => query_has_wildcard(q),
        SetExpr::SetOperation { left, right, .. } => {
            set_expr_has_wildcard(left) || set_expr_has_wildcard(right)
        }
        _ => false,
    }
}

fn select_has_wildcard(select: &Select) -> bool {
    let projection_wild = select.projection.iter().any(|item| {
        matches!(
            item,
            SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(_, _)
        )
    });
    if projection_wild {
        return true;
    }
    // Derived (subquery) tables in FROM.
    select.from.iter().any(|twj| {
        table_factor_has_wildcard(&twj.relation)
            || twj
                .joins
                .iter()
                .any(|j| table_factor_has_wildcard(&j.relation))
    })
}

fn table_factor_has_wildcard(tf: &TableFactor) -> bool {
    match tf {
        TableFactor::Derived { subquery, .. } => query_has_wildcard(subquery),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statement::LoadedStatement;
    use std::path::PathBuf;

    fn entities_dir() -> PathBuf {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../entities"))
    }

    fn catalog() -> Catalog {
        Catalog::load_dir(entities_dir()).expect("entities load")
    }

    #[test]
    fn golden_read_statements_pass_every_rule() {
        let cat = catalog();
        let diags = analyze_catalog(&entities_dir(), &cat).expect("analysis runs");
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:#?}");
    }

    /// A statement skeleton for the hand-built rule tests: a pinned, full-mode
    /// golden_record read with no params, to be bent per test.
    fn read_stmt(sql: &str) -> LoadedStatement {
        LoadedStatement {
            entity: "golden_record".to_string(),
            operation: Operation::Read,
            description: "test".to_string(),
            params: vec![],
            returns: Returns::Entity,
            mode: crate::Mode::Full,
            reads: vec![],
            pin: Pin::Snapshot,
            pin_reason: String::new(),
            sql: sql.to_string(),
            path: PathBuf::from("entities/golden_record/read/test.sql"),
        }
    }

    fn lake_diags(stmt: &LoadedStatement) -> Vec<Diagnostic> {
        let cat = catalog();
        let entity = cat.by_name("golden_record").unwrap();
        let lake = build_store_catalog(&cat, Store::Duckdb);
        let registry = FunctionRegistry::new();
        let provider = FeatherFlowProvider::new(&lake, &registry);
        analyze_statement(stmt, entity, Store::Duckdb, &provider)
    }

    fn codes(diags: &[Diagnostic]) -> Vec<&str> {
        diags.iter().map(|d| d.code).collect()
    }

    /// A hand-built bad statement proves the passes bite: an unpinned `SELECT *`
    /// read raises E006 (no snapshot) and E011 (wildcard).
    #[test]
    fn a_wildcard_unpinned_read_is_rejected() {
        let bad = read_stmt("SELECT * FROM lake.main.golden_records");
        let diags = lake_diags(&bad);
        let codes = codes(&diags);
        assert!(codes.contains(&"E006"), "expected E006, got {codes:?}");
        assert!(codes.contains(&"E011"), "expected E011, got {codes:?}");
    }

    #[test]
    fn a_justified_live_read_passes_without_a_pin() {
        let mut stmt = read_stmt("SELECT entity_id FROM lake.main.golden_records");
        stmt.pin = Pin::Live;
        stmt.pin_reason = "the writer's own in-transaction read".to_string();
        stmt.returns = Returns::Columns(vec![crate::ReturnColumn {
            name: "entity_id".to_string(),
            col_type: ColumnType::Varchar,
            nullable: false,
        }]);
        let diags = lake_diags(&stmt);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:#?}");
    }

    #[test]
    fn a_live_read_carrying_a_pin_is_rejected() {
        let mut stmt =
            read_stmt("SELECT entity_id FROM lake.main.golden_records AT (VERSION => $snapshot)");
        stmt.pin = Pin::Live;
        stmt.pin_reason = "contradiction".to_string();
        stmt.params = vec![crate::StatementParam {
            name: "snapshot".to_string(),
            param_type: ColumnType::Bigint,
            nullable: false,
        }];
        stmt.returns = Returns::Columns(vec![crate::ReturnColumn {
            name: "entity_id".to_string(),
            col_type: ColumnType::Varchar,
            nullable: false,
        }]);
        let diags = lake_diags(&stmt);
        assert!(codes(&diags).contains(&"E006"), "got {diags:#?}");
    }

    #[test]
    fn a_lake_list_parameter_is_rejected() {
        let mut stmt =
            read_stmt("SELECT entity_id FROM lake.main.golden_records AT (VERSION => $snapshot)");
        stmt.params = vec![
            crate::StatementParam {
                name: "snapshot".to_string(),
                param_type: ColumnType::Bigint,
                nullable: false,
            },
            crate::StatementParam {
                name: "keys".to_string(),
                param_type: ColumnType::ListVarchar,
                nullable: false,
            },
        ];
        stmt.returns = Returns::Columns(vec![crate::ReturnColumn {
            name: "entity_id".to_string(),
            col_type: ColumnType::Varchar,
            nullable: false,
        }]);
        let diags = lake_diags(&stmt);
        let e002: Vec<_> = diags.iter().filter(|d| d.code == "E002").collect();
        assert!(
            e002.iter().any(|d| d.message.contains("LIST binding")),
            "expected the lake list-param E002, got {diags:#?}"
        );
    }

    #[test]
    fn a_columns_read_with_a_wrong_type_is_rejected() {
        let mut stmt =
            read_stmt("SELECT entity_id FROM lake.main.golden_records AT (VERSION => $snapshot)");
        stmt.params = vec![crate::StatementParam {
            name: "snapshot".to_string(),
            param_type: ColumnType::Bigint,
            nullable: false,
        }];
        stmt.returns = Returns::Columns(vec![crate::ReturnColumn {
            name: "entity_id".to_string(),
            col_type: ColumnType::Bigint,
            nullable: false,
        }]);
        let diags = lake_diags(&stmt);
        assert!(
            diags
                .iter()
                .any(|d| d.code == "E003" && d.message.contains("declared BIGINT")),
            "expected the type-mismatch E003, got {diags:#?}"
        );
    }

    #[test]
    fn a_columns_read_overclaiming_non_null_is_rejected() {
        // given_name is nullable on the entity; declaring it non-null must fail.
        let mut stmt =
            read_stmt("SELECT given_name FROM lake.main.golden_records AT (VERSION => $snapshot)");
        stmt.params = vec![crate::StatementParam {
            name: "snapshot".to_string(),
            param_type: ColumnType::Bigint,
            nullable: false,
        }];
        stmt.returns = Returns::Columns(vec![crate::ReturnColumn {
            name: "given_name".to_string(),
            col_type: ColumnType::Varchar,
            nullable: false,
        }]);
        let diags = lake_diags(&stmt);
        assert!(
            diags
                .iter()
                .any(|d| d.code == "E003" && d.message.contains("declared non-null")),
            "expected the nullability E003, got {diags:#?}"
        );
    }

    #[test]
    fn a_columns_read_with_wrong_names_is_rejected() {
        let mut stmt =
            read_stmt("SELECT entity_id FROM lake.main.golden_records AT (VERSION => $snapshot)");
        stmt.params = vec![crate::StatementParam {
            name: "snapshot".to_string(),
            param_type: ColumnType::Bigint,
            nullable: false,
        }];
        stmt.returns = Returns::Columns(vec![crate::ReturnColumn {
            name: "record_key".to_string(),
            col_type: ColumnType::Varchar,
            nullable: true,
        }]);
        let diags = lake_diags(&stmt);
        assert!(
            diags
                .iter()
                .any(|d| d.code == "E003" && d.message.contains("declared returns")),
            "expected the name-mismatch E003, got {diags:#?}"
        );
    }

    #[test]
    fn a_control_statement_cannot_return_columns() {
        let cat = catalog();
        let entity = cat.by_name("org").unwrap();
        let pg = build_store_catalog(&cat, Store::Postgres);
        let registry = FunctionRegistry::new();
        let provider = FeatherFlowProvider::new(&pg, &registry);
        let stmt = LoadedStatement {
            entity: "org".to_string(),
            operation: Operation::Read,
            description: "test".to_string(),
            params: vec![],
            returns: Returns::Columns(vec![crate::ReturnColumn {
                name: "name".to_string(),
                col_type: ColumnType::Varchar,
                nullable: false,
            }]),
            mode: crate::Mode::Partial,
            reads: vec![],
            pin: Pin::Snapshot,
            pin_reason: String::new(),
            sql: "SELECT name FROM public.orgs".to_string(),
            path: PathBuf::from("entities/org/read/test.sql"),
        };
        let diags = analyze_statement(&stmt, entity, Store::Postgres, &provider);
        assert!(
            diags
                .iter()
                .any(|d| d.code == "E012" && d.message.contains("lake-read-only")),
            "expected the control-columns E012, got {diags:#?}"
        );
    }

    #[test]
    fn pin_live_outside_a_lake_read_is_rejected() {
        let cat = catalog();
        let entity = cat.by_name("org").unwrap();
        let pg = build_store_catalog(&cat, Store::Postgres);
        let registry = FunctionRegistry::new();
        let provider = FeatherFlowProvider::new(&pg, &registry);
        let stmt = LoadedStatement {
            entity: "org".to_string(),
            operation: Operation::Read,
            description: "test".to_string(),
            params: vec![],
            returns: Returns::Entity,
            mode: crate::Mode::Partial,
            reads: vec![],
            pin: Pin::Live,
            pin_reason: "misplaced".to_string(),
            sql: "SELECT 1 FROM public.orgs".to_string(),
            path: PathBuf::from("entities/org/read/test.sql"),
        };
        let diags = analyze_statement(&stmt, entity, Store::Postgres, &provider);
        assert!(
            diags
                .iter()
                .any(|d| d.code == "E006" && d.message.contains("lake-read concept")),
            "expected the misplaced-pin E006, got {diags:#?}"
        );
    }
}
