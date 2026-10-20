//! The statement layout on disk and its YAML model. Every statement lives under
//! its entity in a folder named for its operation (`create`/`read`/`update`/
//! `delete`), with a `.yaml` beside the `.sql` naming the operation, the typed
//! parameters, what it returns, and a required description.

use std::fs;
use std::path::{Path, PathBuf};

use er_entity_spec::{Catalog, ColumnType};
use serde::Deserialize;

/// A statement's operation, which must match the folder it lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    Create,
    Read,
    Update,
    Delete,
}

impl Operation {
    /// The folder name this operation lives under.
    #[must_use]
    pub fn folder(self) -> &'static str {
        match self {
            Operation::Create => "create",
            Operation::Read => "read",
            Operation::Update => "update",
            Operation::Delete => "delete",
        }
    }

    fn from_folder(name: &str) -> Option<Self> {
        match name {
            "create" => Some(Operation::Create),
            "read" => Some(Operation::Read),
            "update" => Some(Operation::Update),
            "delete" => Some(Operation::Delete),
            _ => None,
        }
    }
}

/// The planning mode: `full` (DataFusion plans the whole statement) or `partial`
/// (name-and-type checked, the oracle supplies the output schema). Partial is the
/// exception the yaml makes visible, never a silent fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Full,
    Partial,
}

/// One declared result column of a `returns: columns` statement: a name, a type
/// from the entity vocabulary, and whether it admits NULL. The emitter builds the
/// bespoke row struct from this declaration; the analyser (E003) proves it against
/// the plan's output schema, and the oracle against the real database.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReturnColumn {
    /// The output column name, matched against the plan's schema in order.
    pub name: String,
    /// The column type from the eight-type vocabulary.
    #[serde(rename = "type")]
    pub col_type: ColumnType,
    /// Whether the column admits NULL. Result columns default NULLABLE (like
    /// entity columns, unlike params): neither database reports result
    /// nullability on its own, so non-null is an opt-in assertion the analyser
    /// and the generated `from_batches` both enforce.
    #[serde(default = "default_true")]
    pub nullable: bool,
}

fn default_true() -> bool {
    true
}

/// What a statement returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Returns {
    /// The named entity's full column set, in order (E003 applies).
    Entity,
    /// A bespoke, ordered, typed column list (E003 applies against the plan).
    Columns(Vec<ReturnColumn>),
    /// A write; no result rows.
    None,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ReturnsRaw {
    Named(String),
    Columns(Vec<ReturnColumnRaw>),
}

/// A returns-list element as parsed: either the typed form or a bare name, which
/// is rejected with a pointed message (a name alone cannot type the generated row).
#[derive(Deserialize)]
#[serde(untagged)]
enum ReturnColumnRaw {
    Typed(ReturnColumn),
    Name(String),
}

/// Whether a lake read is pinned to a snapshot (`AT (VERSION => $snapshot)`, the
/// default and the read API's MVCC rule) or deliberately reads live state. `live`
/// is for the writer's own in-transaction reads and requires a `pin_reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pin {
    /// The statement carries `AT (VERSION => $snapshot)` (E006 enforces it).
    #[default]
    Snapshot,
    /// The statement reads live state; `pin_reason` must say why.
    Live,
}

/// One declared parameter: a name, a type from the entity vocabulary, and whether
/// it is nullable.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatementParam {
    /// The parameter name (matched against the SQL's `$name` placeholders).
    pub name: String,
    /// The parameter type from the eight-type vocabulary.
    #[serde(rename = "type")]
    pub param_type: ColumnType,
    /// Whether the parameter admits NULL. Parameters default to NOT nullable (the
    /// opposite of entity columns): an optional filter says `nullable: true`
    /// explicitly, the way `golden_list`'s `after`/`q` do.
    #[serde(default)]
    pub nullable: bool,
}

/// The parsed `.yaml` beside a statement's `.sql`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatementSpec {
    kind: String,
    operation: Operation,
    description: String,
    #[serde(default)]
    params: Vec<StatementParam>,
    returns: ReturnsRaw,
    #[serde(default)]
    mode: Mode,
    #[serde(default)]
    reads: Vec<String>,
    #[serde(default)]
    pin: Pin,
    #[serde(default)]
    pin_reason: String,
}

/// A loaded statement: its parsed yaml plus the owning entity, the folder it came
/// from, the `.sql` text and the file path (for diagnostics).
#[derive(Debug, Clone)]
pub struct LoadedStatement {
    /// The owning entity's name (the directory under `entities/`).
    pub entity: String,
    /// The operation declared in the yaml (validated to match the folder).
    pub operation: Operation,
    /// Required prose describing the statement.
    pub description: String,
    /// The declared parameters.
    pub params: Vec<StatementParam>,
    /// What the statement returns.
    pub returns: Returns,
    /// The planning mode.
    pub mode: Mode,
    /// Other entities the statement reads (for E009).
    pub reads: Vec<String>,
    /// Snapshot-pinned (the default) or a justified live read.
    pub pin: Pin,
    /// Why a `pin: live` read may skip the snapshot pin (empty otherwise).
    pub pin_reason: String,
    /// The SQL body.
    pub sql: String,
    /// The `.sql` path, repo-relative-ish, for diagnostics.
    pub path: PathBuf,
}

/// A statement load/validation failure.
#[derive(Debug, thiserror::Error)]
pub enum StmtError {
    // `source` is thiserror's source field: named here, not interpolated.
    #[error("reading {path:?}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("parsing {path:?}")]
    Parse {
        path: PathBuf,
        source: serde_norway::Error,
    },
    #[error("{path:?}: {message}")]
    Invalid { path: PathBuf, message: String },
}

fn invalid(path: &Path, message: impl Into<String>) -> StmtError {
    StmtError::Invalid {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

/// Load every statement under `entities_dir`, for the entities in `catalog`. Each
/// `<entity>/<op>/<name>.yaml` is paired with its `<name>.sql`; the yaml's
/// `operation` must match its folder and `kind` must be `statement`.
pub fn load_statements(
    entities_dir: &Path,
    catalog: &Catalog,
) -> Result<Vec<LoadedStatement>, StmtError> {
    let mut out = Vec::new();
    for entity in catalog.entities() {
        for op in [
            Operation::Create,
            Operation::Read,
            Operation::Update,
            Operation::Delete,
        ] {
            let dir = entities_dir.join(&entity.name).join(op.folder());
            if !dir.is_dir() {
                continue;
            }
            let mut yaml_paths: Vec<PathBuf> = fs::read_dir(&dir)
                .map_err(|source| StmtError::Read {
                    path: dir.clone(),
                    source,
                })?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|p| p.extension().is_some_and(|ext| ext == "yaml"))
                .collect();
            yaml_paths.sort();

            for yaml_path in yaml_paths {
                let sql_path = yaml_path.with_extension("sql");
                let spec = load_spec(&yaml_path)?;
                let sql = fs::read_to_string(&sql_path).map_err(|source| StmtError::Read {
                    path: sql_path.clone(),
                    source,
                })?;
                out.push(build_statement(
                    spec,
                    &yaml_path,
                    sql_path,
                    sql,
                    &entity.name,
                    op,
                )?);
            }
        }
    }
    Ok(out)
}

/// Validate a parsed spec + its SQL into a [`LoadedStatement`]. Every structural
/// invariant the yaml alone can prove is checked here (kind, folder match,
/// description, typed returns, pin/pin_reason pairing); store-dependent rules are
/// the analyser's (E0xx).
fn build_statement(
    spec: StatementSpec,
    yaml_path: &Path,
    sql_path: PathBuf,
    sql: String,
    entity: &str,
    op: Operation,
) -> Result<LoadedStatement, StmtError> {
    if spec.kind != "statement" {
        return Err(invalid(
            yaml_path,
            format!("kind must be \"statement\", got {:?}", spec.kind),
        ));
    }
    if spec.operation != op {
        return Err(invalid(
            yaml_path,
            format!(
                "operation {:?} does not match its folder {:?}",
                spec.operation.folder(),
                op.folder()
            ),
        ));
    }
    if spec.description.trim().is_empty() {
        return Err(invalid(yaml_path, "a statement description is required"));
    }
    let returns = match spec.returns {
        ReturnsRaw::Named(s) if s == "entity" => Returns::Entity,
        ReturnsRaw::Named(s) if s == "none" => Returns::None,
        ReturnsRaw::Named(s) => {
            return Err(invalid(
                yaml_path,
                format!("returns must be `entity`, `none`, or a typed column list, got {s:?}"),
            ));
        }
        ReturnsRaw::Columns(raw) => {
            let mut cols = Vec::with_capacity(raw.len());
            for item in raw {
                match item {
                    ReturnColumnRaw::Typed(col) => cols.push(col),
                    ReturnColumnRaw::Name(name) => {
                        return Err(invalid(
                            yaml_path,
                            format!(
                                "declare each returns column as `{{name, type}}` — a bare \
                                 name cannot type the generated row (got {name:?})"
                            ),
                        ));
                    }
                }
            }
            let mut seen = std::collections::BTreeSet::new();
            for col in &cols {
                if !seen.insert(col.name.as_str()) {
                    return Err(invalid(
                        yaml_path,
                        format!("duplicate returns column {:?}", col.name),
                    ));
                }
            }
            Returns::Columns(cols)
        }
    };
    if spec.operation == Operation::Read && returns == Returns::None {
        return Err(invalid(
            yaml_path,
            "a read returns rows; declare `entity` or a typed column list",
        ));
    }
    match spec.pin {
        Pin::Live if spec.pin_reason.trim().is_empty() => {
            return Err(invalid(
                yaml_path,
                "`pin: live` requires a pin_reason justifying the unpinned read",
            ));
        }
        Pin::Snapshot if !spec.pin_reason.trim().is_empty() => {
            return Err(invalid(
                yaml_path,
                "pin_reason is only for `pin: live`; a pinned read needs no justification",
            ));
        }
        _ => {}
    }
    Ok(LoadedStatement {
        entity: entity.to_string(),
        operation: spec.operation,
        description: spec.description,
        params: spec.params,
        returns,
        mode: spec.mode,
        reads: spec.reads,
        pin: spec.pin,
        pin_reason: spec.pin_reason,
        sql,
        path: sql_path,
    })
}

fn load_spec(path: &Path) -> Result<StatementSpec, StmtError> {
    let text = fs::read_to_string(path).map_err(|source| StmtError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    serde_norway::from_str(&text).map_err(|source| StmtError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

/// The operation a folder name maps to, if any (exposed for the folder/kind check).
#[must_use]
pub fn operation_for_folder(name: &str) -> Option<Operation> {
    Operation::from_folder(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(yaml: &str, op: Operation) -> Result<LoadedStatement, StmtError> {
        let spec: StatementSpec = serde_norway::from_str(yaml).expect("spec parses");
        build_statement(
            spec,
            Path::new("entities/x/read/t.yaml"),
            PathBuf::from("entities/x/read/t.sql"),
            "SELECT 1".to_string(),
            "x",
            op,
        )
    }

    fn message(err: StmtError) -> String {
        match err {
            StmtError::Invalid { message, .. } => message,
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn a_typed_returns_list_parses_with_nullable_defaulting_true() {
        let stmt = build(
            "kind: statement\noperation: read\ndescription: d\nreturns:\n  - { name: record_key, type: varchar, nullable: false }\n  - { name: n, type: bigint }\n",
            Operation::Read,
        )
        .expect("valid statement");
        let Returns::Columns(cols) = &stmt.returns else {
            panic!("expected columns, got {:?}", stmt.returns);
        };
        assert_eq!(cols[0].name, "record_key");
        assert!(!cols[0].nullable);
        assert_eq!(cols[1].name, "n");
        assert!(cols[1].nullable, "returns columns default nullable");
        assert_eq!(stmt.pin, Pin::Snapshot);
    }

    #[test]
    fn a_bare_name_returns_list_is_rejected() {
        let err = build(
            "kind: statement\noperation: read\ndescription: d\nreturns: [record_key]\n",
            Operation::Read,
        )
        .expect_err("bare names cannot type the row");
        assert!(message(err).contains("a bare name cannot type the generated row"));
    }

    #[test]
    fn a_read_returning_none_is_rejected() {
        let err = build(
            "kind: statement\noperation: read\ndescription: d\nreturns: none\n",
            Operation::Read,
        )
        .expect_err("a read returns rows");
        assert!(message(err).contains("a read returns rows"));
    }

    #[test]
    fn a_duplicate_returns_column_is_rejected() {
        let err = build(
            "kind: statement\noperation: read\ndescription: d\nreturns:\n  - { name: a, type: varchar }\n  - { name: a, type: bigint }\n",
            Operation::Read,
        )
        .expect_err("duplicate column");
        assert!(message(err).contains("duplicate returns column"));
    }

    #[test]
    fn pin_live_without_a_reason_is_rejected() {
        let err = build(
            "kind: statement\noperation: read\ndescription: d\nreturns: entity\npin: live\n",
            Operation::Read,
        )
        .expect_err("live needs a reason");
        assert!(message(err).contains("requires a pin_reason"));
    }

    #[test]
    fn a_pin_reason_without_pin_live_is_rejected() {
        let err = build(
            "kind: statement\noperation: read\ndescription: d\nreturns: entity\npin_reason: why\n",
            Operation::Read,
        )
        .expect_err("reason only justifies live");
        assert!(message(err).contains("only for `pin: live`"));
    }

    #[test]
    fn pin_live_with_a_reason_loads() {
        let stmt = build(
            "kind: statement\noperation: read\ndescription: d\nreturns: entity\npin: live\npin_reason: \"the writer's own in-transaction read\"\n",
            Operation::Read,
        )
        .expect("valid live read");
        assert_eq!(stmt.pin, Pin::Live);
        assert_eq!(stmt.pin_reason, "the writer's own in-transaction read");
    }
}
