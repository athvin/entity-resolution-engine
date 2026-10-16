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

/// What a statement returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Returns {
    /// The named entity's full column set, in order (E003 applies).
    Entity,
    /// A bespoke, ordered column list.
    Columns(Vec<String>),
    /// A write; no result rows.
    None,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ReturnsRaw {
    Named(String),
    Columns(Vec<String>),
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
    /// The SQL body.
    pub sql: String,
    /// The `.sql` path, repo-relative-ish, for diagnostics.
    pub path: PathBuf,
}

/// A statement load/validation failure.
#[derive(Debug, thiserror::Error)]
pub enum StmtError {
    #[error("reading {path:?}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("parsing {path:?}: {source}")]
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
                if spec.kind != "statement" {
                    return Err(invalid(
                        &yaml_path,
                        format!("kind must be \"statement\", got {:?}", spec.kind),
                    ));
                }
                if spec.operation != op {
                    return Err(invalid(
                        &yaml_path,
                        format!(
                            "operation {:?} does not match its folder {:?}",
                            spec.operation.folder(),
                            op.folder()
                        ),
                    ));
                }
                if spec.description.trim().is_empty() {
                    return Err(invalid(&yaml_path, "a statement description is required"));
                }
                let sql = fs::read_to_string(&sql_path).map_err(|source| StmtError::Read {
                    path: sql_path.clone(),
                    source,
                })?;
                let returns = match spec.returns {
                    ReturnsRaw::Named(s) if s == "entity" => Returns::Entity,
                    ReturnsRaw::Named(s) if s == "none" => Returns::None,
                    ReturnsRaw::Named(s) => {
                        return Err(invalid(
                            &yaml_path,
                            format!(
                                "returns must be `entity`, `none`, or a column list, got {s:?}"
                            ),
                        ));
                    }
                    ReturnsRaw::Columns(cols) => Returns::Columns(cols),
                };
                out.push(LoadedStatement {
                    entity: entity.name.clone(),
                    operation: spec.operation,
                    description: spec.description,
                    params: spec.params,
                    returns,
                    mode: spec.mode,
                    reads: spec.reads,
                    sql,
                    path: sql_path,
                });
            }
        }
    }
    Ok(out)
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
