//! The entity passes. Each loaded statement goes through the lake rewrite, is
//! planned against the store catalog, and then checked by the E0xx rules. A
//! failing check is a [`Diagnostic`] naming its code, the statement path and the
//! cause. Type-level checks that need a real database (the oracle) land later;
//! these are the static rules that need only the plan and the entity.

use std::path::PathBuf;

use datafusion_common::arrow::datatypes::DataType;
use er_entity_spec::{Catalog, ColumnType, Entity, Store};
use ff_analysis::sqlparser::ast::{
    Assignment, AssignmentTarget, Expr, Insert, MergeAction, Query, Select, SelectItem, SetExpr,
    Statement, TableFactor, Value,
};
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
    catalog: &Catalog,
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

    // Partial mode: the visible exception DataFusion's PLANNER cannot take (a
    // MERGE, a write with ON CONFLICT ... WHERE, FOR UPDATE SKIP LOCKED, a
    // DuckDB-only read). The parser usually still can — so a partial statement
    // gets the full AST passes (E004/E005/E010/E011/E012) whenever it parses,
    // and only falls back to the text heuristics when it does not. Parameters
    // are covered by text (E002; no plan, no inference), and the oracle supplies
    // the output schema.
    if stmt.mode == Mode::Partial {
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
        let dialect = dialect_for(store);
        match Parser::parse_sql(dialect.as_ref(), &rewritten.sql) {
            Ok(mut stmts) if !stmts.is_empty() => {
                diags.extend(ast_checks(stmt, entity, catalog, store, &stmts.remove(0)));
            }
            _ => {
                // Truly unparseable: the text heuristics hold the line.
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

    // The AST passes: E011 (no wildcard), E012 (verb matches folder), E004
    // (INSERT coverage/arity), E005 (assignments), E009 (declared reads), E010
    // (accepted values).
    diags.extend(ast_checks(stmt, entity, catalog, store, &parsed));

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

    // E003 — the statement's output equals its declaration, names in order, each
    // plan type compatible with the declared type, and a declared non-null column
    // must be provably non-null in the plan. The declaration is the entity's
    // column set (`returns: entity`) or the declared typed list (`returns:
    // columns`); both get the same type/nullability proof (the oracle still holds
    // the exact DuckDB-type line).
    match &stmt.returns {
        Returns::Entity => {
            let fields = plan.schema().fields();
            let actual: Vec<String> = fields.iter().map(|f| f.name().clone()).collect();
            let expected: Vec<String> = entity.columns.iter().map(|c| c.name.clone()).collect();
            if actual == expected {
                for (col, field) in entity.columns.iter().zip(fields.iter()) {
                    if !arrow_compatible(col.col_type, field.data_type()) {
                        diags.push(Diagnostic::new(
                            "E003",
                            &stmt.path,
                            format!(
                                "column {}: entity {} declares {} but the plan output is {:?}",
                                col.name,
                                entity.name,
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
                                "column {} is NOT NULL on entity {} but the plan cannot prove it \
                                 (an outer join or expression on its path)",
                                col.name, entity.name
                            ),
                        ));
                    }
                }
            } else {
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
        diags.extend(analyze_statement(stmt, entity, store, provider, catalog));
    }
    Ok(diags)
}

// ── structural helpers over the sqlparser AST ──────────────────────────────

/// The AST passes shared by full and (parseable) partial statements: E011 (no
/// wildcard), E012 (verb vs folder), E004 (INSERT coverage/arity), E005
/// (assignment legality) and E010 (accepted-values literals).
fn ast_checks(
    stmt: &LoadedStatement,
    entity: &Entity,
    catalog: &Catalog,
    store: Store,
    parsed: &Statement,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    if statement_has_wildcard(parsed) {
        diags.push(Diagnostic::new(
            "E011",
            &stmt.path,
            "SELECT * is forbidden; name every column so an added column cannot change the shape",
        ));
    }
    if let Some(code) = verb_mismatch(parsed, stmt.operation) {
        diags.push(Diagnostic::new("E012", &stmt.path, code));
    }
    match parsed {
        Statement::Insert(insert) => {
            diags.extend(insert_checks(stmt, entity, insert));
        }
        Statement::Update { assignments, .. } => {
            diags.extend(assignment_checks(stmt, entity, assignments));
            diags.extend(accepted_values_assignments(stmt, entity, assignments));
        }
        Statement::Merge { clauses, .. } => {
            for clause in clauses {
                match &clause.action {
                    MergeAction::Update { assignments } => {
                        diags.extend(assignment_checks(stmt, entity, assignments));
                        diags.extend(accepted_values_assignments(stmt, entity, assignments));
                    }
                    MergeAction::Insert(insert) => {
                        if !insert.columns.is_empty() {
                            diags.extend(insert_coverage_checks(
                                stmt,
                                entity,
                                &insert
                                    .columns
                                    .iter()
                                    .map(|c| c.value.to_lowercase())
                                    .collect::<Vec<_>>(),
                            ));
                        }
                    }
                    MergeAction::Delete => {}
                }
            }
        }
        _ => {}
    }
    diags.extend(accepted_values_comparisons(stmt, entity, parsed));
    diags.extend(reads_checks(stmt, entity, catalog, store, parsed));
    diags
}

/// E009 — the statement references only relations of its own store, and every
/// entity beyond its own is named under `reads:` (the declaration the differ's
/// impact report and the oracle's DDL set rely on). Unqualified references are
/// session temps or CTE names — scratch, governed elsewhere.
fn reads_checks(
    stmt: &LoadedStatement,
    entity: &Entity,
    catalog: &Catalog,
    store: Store,
    parsed: &Statement,
) -> Vec<Diagnostic> {
    use std::ops::ControlFlow;

    let mut diags = Vec::new();
    // A reads: entry must name a real entity of the statement's store.
    for name in &stmt.reads {
        match catalog.by_name(name) {
            None => diags.push(Diagnostic::new(
                "E009",
                &stmt.path,
                format!("reads: names {name:?}, which is not an entity"),
            )),
            Some(read) if !read.has_store(store) => diags.push(Diagnostic::new(
                "E009",
                &stmt.path,
                format!("reads: names {name:?}, which does not live in this statement's store"),
            )),
            Some(_) => {}
        }
    }

    let mut refs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let _ = ff_analysis::sqlparser::ast::visit_relations(parsed, |relation| {
        refs.insert(relation.to_string().to_lowercase());
        ControlFlow::<()>::Continue(())
    });

    let qualifier = format!("{}.", store.qualifier());
    let own = entity.relation.to_lowercase();
    for full in refs {
        let Some(rel) = full.strip_prefix(&qualifier) else {
            if full.contains('.') {
                diags.push(Diagnostic::new(
                    "E009",
                    &stmt.path,
                    format!(
                        "references {full}, outside the statement's {} store",
                        store.qualifier()
                    ),
                ));
            }
            continue;
        };
        if rel == own {
            continue;
        }
        match catalog
            .entities()
            .iter()
            .find(|e| e.relation.to_lowercase() == rel && e.has_store(store))
        {
            None => diags.push(Diagnostic::new(
                "E009",
                &stmt.path,
                format!("references {full}, which no entity of this store declares"),
            )),
            Some(read) if !stmt.reads.iter().any(|r| r == &read.name) => {
                diags.push(Diagnostic::new(
                    "E009",
                    &stmt.path,
                    format!(
                        "reads entity {} ({full}); declare it under reads: so the impact \
                         surface is explicit",
                        read.name
                    ),
                ));
            }
            Some(_) => {}
        }
    }
    diags
}

/// The accepted-values columns of an entity, lowercased name -> allowed set.
fn accepted_map(entity: &Entity) -> std::collections::BTreeMap<String, &Vec<String>> {
    entity
        .columns
        .iter()
        .filter_map(|c| {
            c.accepted_values
                .as_ref()
                .map(|vals| (c.name.to_lowercase(), vals))
        })
        .collect()
}

/// E005 — every assigned column exists on the entity, and a key column is never
/// assigned (an UPDATE/MERGE may not rewrite identity).
fn assignment_checks(
    stmt: &LoadedStatement,
    entity: &Entity,
    assignments: &[Assignment],
) -> Vec<Diagnostic> {
    let columns: std::collections::BTreeSet<String> = entity
        .columns
        .iter()
        .map(|c| c.name.to_lowercase())
        .collect();
    let keys: std::collections::BTreeSet<String> =
        entity.key.iter().map(|k| k.to_lowercase()).collect();
    let mut diags = Vec::new();
    for name in assignment_targets(assignments) {
        if !columns.contains(&name) {
            diags.push(Diagnostic::new(
                "E005",
                &stmt.path,
                format!(
                    "assigns {name}, which entity {} does not declare",
                    entity.name
                ),
            ));
        } else if keys.contains(&name) {
            diags.push(Diagnostic::new(
                "E005",
                &stmt.path,
                format!("assigns the key column {name}; an entity's identity is never rewritten"),
            ));
        }
    }
    diags
}

/// The lowercased column names an assignment list writes.
fn assignment_targets(assignments: &[Assignment]) -> Vec<String> {
    let mut out = Vec::new();
    for assignment in assignments {
        match &assignment.target {
            AssignmentTarget::ColumnName(name) => {
                if let Some(last) = name.0.last() {
                    out.push(last.to_string().trim_matches('"').to_lowercase());
                }
            }
            AssignmentTarget::Tuple(names) => {
                for name in names {
                    if let Some(last) = name.0.last() {
                        out.push(last.to_string().trim_matches('"').to_lowercase());
                    }
                }
            }
        }
    }
    out
}

/// E004 — an INSERT supplies every NOT NULL column that has no default: named
/// columns are checked as a set; a positional insert is checked by arity
/// against the entity's full column list.
fn insert_checks(stmt: &LoadedStatement, entity: &Entity, insert: &Insert) -> Vec<Diagnostic> {
    if !insert.columns.is_empty() {
        let named: Vec<String> = insert
            .columns
            .iter()
            .map(|c| c.value.to_lowercase())
            .collect();
        return insert_coverage_checks(stmt, entity, &named);
    }
    // Positional: the supplied arity must equal the entity's column count.
    let Some(source) = &insert.source else {
        return Vec::new();
    };
    let supplied = match source.body.as_ref() {
        SetExpr::Values(values) => values.rows.first().map(Vec::len),
        SetExpr::Select(select) => Some(select.projection.len()),
        _ => None,
    };
    match supplied {
        Some(n) if n != entity.columns.len() => vec![Diagnostic::new(
            "E004",
            &stmt.path,
            format!(
                "positional INSERT supplies {n} values but entity {} has {} columns; \
                 name the columns or supply every one",
                entity.name,
                entity.columns.len()
            ),
        )],
        _ => Vec::new(),
    }
}

/// The named-columns half of E004: every named column exists, and every
/// NOT NULL column without a default is among them.
fn insert_coverage_checks(
    stmt: &LoadedStatement,
    entity: &Entity,
    named: &[String],
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let columns: std::collections::BTreeSet<String> = entity
        .columns
        .iter()
        .map(|c| c.name.to_lowercase())
        .collect();
    let named_set: std::collections::BTreeSet<&str> = named.iter().map(String::as_str).collect();
    for name in named {
        if !columns.contains(name) {
            diags.push(Diagnostic::new(
                "E004",
                &stmt.path,
                format!(
                    "inserts into {name}, which entity {} does not declare",
                    entity.name
                ),
            ));
        }
    }
    for column in &entity.columns {
        // A serial column's value is database-supplied, like a default.
        if !column.nullable
            && column.default.is_none()
            && !column.serial
            && !named_set.contains(column.name.to_lowercase().as_str())
        {
            diags.push(Diagnostic::new(
                "E004",
                &stmt.path,
                format!(
                    "column {} is NOT NULL with no default but the INSERT does not supply it",
                    column.name
                ),
            ));
        }
    }
    diags
}

/// The E010 assignment half: a string literal assigned into an accepted-values
/// column must be in the set.
fn accepted_values_assignments(
    stmt: &LoadedStatement,
    entity: &Entity,
    assignments: &[Assignment],
) -> Vec<Diagnostic> {
    let accepted = accepted_map(entity);
    if accepted.is_empty() {
        return Vec::new();
    }
    let mut diags = Vec::new();
    for assignment in assignments {
        let targets = assignment_targets(std::slice::from_ref(assignment));
        let Some(name) = targets.first() else {
            continue;
        };
        if let (Some(allowed), Some(literal)) =
            (accepted.get(name), string_literal(&assignment.value))
        {
            if !allowed.iter().any(|v| v == &literal) {
                diags.push(Diagnostic::new(
                    "E010",
                    &stmt.path,
                    format!(
                        "assigns '{literal}' to {name}, outside its accepted values {allowed:?}"
                    ),
                ));
            }
        }
    }
    diags
}

/// The E010 comparison half: walk the statement's predicates (WHERE / MERGE ON /
/// clause predicates) for `<column> = '<literal>'` against accepted-values
/// columns, plus positional/named INSERT VALUES rows.
fn accepted_values_comparisons(
    stmt: &LoadedStatement,
    entity: &Entity,
    parsed: &Statement,
) -> Vec<Diagnostic> {
    let accepted = accepted_map(entity);
    if accepted.is_empty() {
        return Vec::new();
    }
    let mut diags = Vec::new();
    let mut check = |name: &str, literal: &str| {
        if let Some(allowed) = accepted.get(name) {
            if !allowed.iter().any(|v| v == literal) {
                diags.push(Diagnostic::new(
                    "E010",
                    &stmt.path,
                    format!("'{literal}' is outside {name}'s accepted values {allowed:?}"),
                ));
            }
        }
    };
    match parsed {
        Statement::Query(query) => comparisons_in_query(query, &mut check),
        Statement::Update { selection, .. }
        | Statement::Delete(ff_analysis::sqlparser::ast::Delete { selection, .. }) => {
            if let Some(expr) = selection {
                comparisons_in_expr(expr, &mut check);
            }
        }
        Statement::Merge {
            on,
            clauses,
            source,
            ..
        } => {
            comparisons_in_expr(on, &mut check);
            for clause in clauses {
                if let Some(predicate) = &clause.predicate {
                    comparisons_in_expr(predicate, &mut check);
                }
            }
            if let TableFactor::Derived { subquery, .. } = source {
                comparisons_in_query(subquery, &mut check);
            }
        }
        Statement::Insert(insert) => {
            // VALUES rows: zip each row against the named columns (or the
            // entity's full list when positional).
            let names: Vec<String> = if insert.columns.is_empty() {
                entity
                    .columns
                    .iter()
                    .map(|c| c.name.to_lowercase())
                    .collect()
            } else {
                insert
                    .columns
                    .iter()
                    .map(|c| c.value.to_lowercase())
                    .collect()
            };
            if let Some(source) = &insert.source {
                if let SetExpr::Values(values) = source.body.as_ref() {
                    for row in &values.rows {
                        for (name, expr) in names.iter().zip(row.iter()) {
                            if let Some(literal) = string_literal(expr) {
                                check(name, &literal);
                            }
                        }
                    }
                } else {
                    comparisons_in_query(source, &mut check);
                }
            }
        }
        _ => {}
    }
    diags
}

/// Walk a query's WHERE/HAVING (top level, set-op arms and CTEs) for
/// column-vs-literal equality comparisons.
fn comparisons_in_query(query: &Query, check: &mut dyn FnMut(&str, &str)) {
    if let Some(with) = &query.with {
        for cte in &with.cte_tables {
            comparisons_in_query(&cte.query, check);
        }
    }
    comparisons_in_set_expr(&query.body, check);
}

fn comparisons_in_set_expr(body: &SetExpr, check: &mut dyn FnMut(&str, &str)) {
    match body {
        SetExpr::Select(select) => {
            if let Some(selection) = &select.selection {
                comparisons_in_expr(selection, check);
            }
            if let Some(having) = &select.having {
                comparisons_in_expr(having, check);
            }
            for twj in &select.from {
                if let TableFactor::Derived { subquery, .. } = &twj.relation {
                    comparisons_in_query(subquery, check);
                }
            }
        }
        SetExpr::Query(q) => comparisons_in_query(q, check),
        SetExpr::SetOperation { left, right, .. } => {
            comparisons_in_set_expr(left, check);
            comparisons_in_set_expr(right, check);
        }
        _ => {}
    }
}

/// Recurse an expression for `<column> =/<> '<literal>'` (either orientation),
/// reporting through `check`.
fn comparisons_in_expr(expr: &Expr, check: &mut dyn FnMut(&str, &str)) {
    use ff_analysis::sqlparser::ast::BinaryOperator;
    if let Expr::BinaryOp { left, op, right } = expr {
        if matches!(op, BinaryOperator::Eq | BinaryOperator::NotEq) {
            let pair = match (column_name(left), string_literal(right)) {
                (Some(name), Some(literal)) => Some((name, literal)),
                _ => match (column_name(right), string_literal(left)) {
                    (Some(name), Some(literal)) => Some((name, literal)),
                    _ => None,
                },
            };
            if let Some((name, literal)) = pair {
                check(&name, &literal);
            }
        }
        comparisons_in_expr(left, check);
        comparisons_in_expr(right, check);
    } else if let Expr::Nested(inner) = expr {
        comparisons_in_expr(inner, check);
    }
}

/// The lowercased column name of an identifier expression, if it is one.
fn column_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Identifier(ident) => Some(ident.value.to_lowercase()),
        Expr::CompoundIdentifier(parts) => parts.last().map(|i| i.value.to_lowercase()),
        _ => None,
    }
}

/// The string literal an expression carries, if it is one.
fn string_literal(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Value(value) => match &value.value {
            Value::SingleQuotedString(s) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

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
/// when it mismatches, `None` when it is fine. Mirrors the text rule: `read`
/// holds SELECT and COPY TO; `create` holds INSERT, COPY FROM and a MERGE that
/// inserts; `update` holds UPDATE and a MERGE that only updates.
fn verb_mismatch(stmt: &Statement, op: Operation) -> Option<String> {
    let ok = match op {
        Operation::Read => matches!(stmt, Statement::Query(_) | Statement::Copy { .. }),
        Operation::Create => matches!(
            stmt,
            Statement::Insert(_) | Statement::Copy { .. } | Statement::Merge { .. }
        ),
        Operation::Update => {
            matches!(stmt, Statement::Update { .. } | Statement::Merge { .. })
        }
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
        analyze_statement(stmt, entity, Store::Duckdb, &provider, &cat)
    }

    fn codes(diags: &[Diagnostic]) -> Vec<&str> {
        diags.iter().map(|d| d.code).collect()
    }

    /// A partial-mode write statement skeleton over golden_record, bent per test.
    fn partial_write(op: Operation, sql: &str) -> LoadedStatement {
        LoadedStatement {
            entity: "golden_record".to_string(),
            operation: op,
            description: "test".to_string(),
            params: vec![],
            returns: Returns::None,
            mode: crate::Mode::Partial,
            reads: vec![],
            pin: Pin::Snapshot,
            pin_reason: String::new(),
            sql: sql.to_string(),
            path: PathBuf::from("entities/golden_record/update/test.sql"),
        }
    }

    fn golden_diags(stmt: &LoadedStatement) -> Vec<Diagnostic> {
        let cat = catalog();
        let entity = cat.by_name("golden_record").unwrap();
        let lake = build_store_catalog(&cat, Store::Duckdb);
        let registry = FunctionRegistry::new();
        let provider = FeatherFlowProvider::new(&lake, &registry);
        analyze_statement(stmt, entity, Store::Duckdb, &provider, &cat)
    }

    #[test]
    fn an_update_assigning_an_unknown_column_is_rejected() {
        let stmt = partial_write(
            Operation::Update,
            "UPDATE lake.main.golden_records SET nickname = 'x'",
        );
        let diags = golden_diags(&stmt);
        assert!(
            diags
                .iter()
                .any(|d| d.code == "E005" && d.message.contains("does not declare")),
            "expected the unknown-column E005, got {diags:#?}"
        );
    }

    #[test]
    fn an_update_assigning_the_key_is_rejected() {
        let stmt = partial_write(
            Operation::Update,
            "UPDATE lake.main.golden_records SET entity_id = 'x'",
        );
        let diags = golden_diags(&stmt);
        assert!(
            diags
                .iter()
                .any(|d| d.code == "E005" && d.message.contains("key column")),
            "expected the key-assignment E005, got {diags:#?}"
        );
    }

    #[test]
    fn a_merge_assigning_the_key_is_rejected() {
        let stmt = partial_write(
            Operation::Update,
            "MERGE INTO lake.main.golden_records t USING (SELECT 'a' AS entity_id) s \
             ON t.entity_id = s.entity_id \
             WHEN MATCHED THEN UPDATE SET entity_id = s.entity_id",
        );
        let diags = golden_diags(&stmt);
        assert!(
            diags
                .iter()
                .any(|d| d.code == "E005" && d.message.contains("key column")),
            "expected the MERGE key-assignment E005, got {diags:#?}"
        );
        // ...and the MERGE itself is a legal verb for the update folder (no E012).
        assert!(
            !diags.iter().any(|d| d.code == "E012"),
            "a MERGE under update/ must pass the verb rule, got {diags:#?}"
        );
    }

    #[test]
    fn a_positional_insert_with_wrong_arity_is_rejected() {
        let stmt = LoadedStatement {
            operation: Operation::Create,
            path: PathBuf::from("entities/golden_record/create/test.sql"),
            ..partial_write(
                Operation::Create,
                "INSERT INTO lake.main.golden_records VALUES ('a', 'b')",
            )
        };
        let diags = golden_diags(&stmt);
        assert!(
            diags
                .iter()
                .any(|d| d.code == "E004" && d.message.contains("positional INSERT")),
            "expected the arity E004, got {diags:#?}"
        );
    }

    #[test]
    fn a_named_insert_missing_a_not_null_column_is_rejected() {
        // golden_record: entity_id / survivorship_version / assembled_at are
        // NOT NULL with no default; naming only entity_id must flag the others.
        let stmt = LoadedStatement {
            operation: Operation::Create,
            path: PathBuf::from("entities/golden_record/create/test.sql"),
            ..partial_write(
                Operation::Create,
                "INSERT INTO lake.main.golden_records (entity_id) VALUES ('a')",
            )
        };
        let diags = golden_diags(&stmt);
        assert!(
            diags.iter().any(|d| d.code == "E004"
                && d.message.contains("survivorship_version")
                && d.message.contains("NOT NULL with no default")),
            "expected the coverage E004, got {diags:#?}"
        );
    }

    fn org_diags(stmt: &LoadedStatement) -> Vec<Diagnostic> {
        let cat = catalog();
        let entity = cat.by_name("org").unwrap();
        let pg = build_store_catalog(&cat, Store::Postgres);
        let registry = FunctionRegistry::new();
        let provider = FeatherFlowProvider::new(&pg, &registry);
        analyze_statement(stmt, entity, Store::Postgres, &provider, &cat)
    }

    #[test]
    fn an_accepted_values_violation_is_rejected_in_every_context() {
        // org.state accepts [provisioning, active, suspended, purging].
        let mut base = partial_write(Operation::Update, "");
        base.entity = "org".to_string();
        base.path = PathBuf::from("entities/org/update/test.sql");

        // 1. assignment
        let mut assign = base.clone();
        assign.sql = "UPDATE public.orgs SET state = 'zombie'".to_string();
        assert!(
            org_diags(&assign)
                .iter()
                .any(|d| d.code == "E010" && d.message.contains("zombie")),
            "assignment context"
        );

        // 2. WHERE comparison
        let mut filter = base.clone();
        filter.operation = Operation::Read;
        filter.returns = Returns::Entity;
        filter.path = PathBuf::from("entities/org/read/test.sql");
        filter.sql =
            "SELECT 1 FROM public.orgs WHERE state = 'zombie' FOR UPDATE SKIP LOCKED".to_string();
        assert!(
            org_diags(&filter)
                .iter()
                .any(|d| d.code == "E010" && d.message.contains("zombie")),
            "comparison context"
        );

        // 3. INSERT literal (named columns)
        let mut insert = base.clone();
        insert.operation = Operation::Create;
        insert.path = PathBuf::from("entities/org/create/test.sql");
        insert.sql = "INSERT INTO public.orgs (name, state) VALUES ('x', 'zombie') \
                      ON CONFLICT (name) DO NOTHING"
            .to_string();
        assert!(
            org_diags(&insert)
                .iter()
                .any(|d| d.code == "E010" && d.message.contains("zombie")),
            "insert context"
        );

        // ...and an accepted literal passes all three.
        let mut fine = base;
        fine.sql = "UPDATE public.orgs SET state = 'suspended'".to_string();
        assert!(
            !org_diags(&fine).iter().any(|d| d.code == "E010"),
            "an accepted literal must pass"
        );
    }

    #[test]
    fn an_undeclared_cross_entity_read_is_rejected() {
        // The override shape without its reads: declaration.
        let stmt = partial_write(
            Operation::Update,
            "UPDATE lake.main.golden_records SET email = (SELECT o.value \
             FROM lake.main.golden_overrides o WHERE o.entity_id = golden_records.entity_id)",
        );
        let diags = golden_diags(&stmt);
        assert!(
            diags.iter().any(|d| d.code == "E009"
                && d.message.contains("golden_override")
                && d.message.contains("reads:")),
            "expected the undeclared-read E009, got {diags:#?}"
        );

        // ...and declaring it passes.
        let mut declared = partial_write(
            Operation::Update,
            "UPDATE lake.main.golden_records SET email = (SELECT o.value \
             FROM lake.main.golden_overrides o WHERE o.entity_id = golden_records.entity_id)",
        );
        declared.reads = vec!["golden_override".to_string()];
        assert!(
            !golden_diags(&declared).iter().any(|d| d.code == "E009"),
            "a declared read must pass"
        );
    }

    #[test]
    fn a_reads_entry_naming_no_entity_is_rejected() {
        let mut stmt = partial_write(
            Operation::Update,
            "UPDATE lake.main.golden_records SET email = 'x'",
        );
        stmt.reads = vec!["golden_overlays".to_string()];
        let diags = golden_diags(&stmt);
        assert!(
            diags
                .iter()
                .any(|d| d.code == "E009" && d.message.contains("not an entity")),
            "expected the unknown-reads E009, got {diags:#?}"
        );
    }

    #[test]
    fn a_cross_store_reference_is_rejected() {
        let stmt = partial_write(
            Operation::Update,
            "UPDATE lake.main.golden_records SET email = (SELECT name FROM public.orgs LIMIT 1)",
        );
        let diags = golden_diags(&stmt);
        assert!(
            diags
                .iter()
                .any(|d| d.code == "E009" && d.message.contains("outside the statement's")),
            "expected the cross-store E009, got {diags:#?}"
        );
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

    /// `returns: entity` gets the same type/nullability proof as a typed column
    /// list: an expression that loses a NOT NULL column's provability fails E003
    /// even when every name lines up.
    #[test]
    fn an_entity_read_losing_non_null_provability_is_rejected() {
        let mut stmt = read_stmt(
            "SELECT NULLIF(entity_id, '') AS entity_id, given_name, family_name, email, \
             phone_e164, addr_number, addr_street, addr_unit, addr_city, addr_region, \
             addr_postal, birth_date, metadata, survivorship_version, assembled_at \
             FROM lake.main.golden_records AT (VERSION => $snapshot)",
        );
        stmt.params = vec![crate::StatementParam {
            name: "snapshot".to_string(),
            param_type: ColumnType::Bigint,
            nullable: false,
        }];
        let diags = lake_diags(&stmt);
        assert!(
            diags
                .iter()
                .any(|d| d.code == "E003" && d.message.contains("NOT NULL on entity")),
            "expected the entity nullability E003, got {diags:#?}"
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
        let diags = analyze_statement(&stmt, entity, Store::Postgres, &provider, &cat);
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
        let diags = analyze_statement(&stmt, entity, Store::Postgres, &provider, &cat);
        assert!(
            diags
                .iter()
                .any(|d| d.code == "E006" && d.message.contains("lake-read concept")),
            "expected the misplaced-pin E006, got {diags:#?}"
        );
    }
}
