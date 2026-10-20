//! The [`Catalog`]: the structurally coherent set of entities, loaded from an
//! `entities/` tree. Loading is where every deliberate, loud default lives — a
//! missing `generate` block, a key column that is not a column, a duplicate
//! relation each fail here with a named cause, never a silent guess.
//!
//! What loading checks is deliberately narrow: only the invariants a [`Catalog`]'s
//! own consumers would otherwise trip over — a duplicate column name would emit a
//! duplicate struct field, a key naming no column would emit an unsatisfiable
//! primary key. The *conventions* (the V-rules: `id` is the key, the universal
//! timestamps, foreign-key naming, what each store will honour) live in
//! `er-entity-lint`, which reports every violation at once instead of the first
//! and carries each rule's code and reasoning as data.
//!
//! The consequence worth knowing: [`Catalog::load_dir`] no longer enforces the
//! conventions, so anything about to *write a generated artifact* should load
//! through `er_entity_lint::load_linted` instead. `er-entity verify` does, and
//! `er-entity-lint`'s own `tree_is_clean` test holds the committed tree to them
//! for every consumer that still calls `load_dir` directly.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::entity::Entity;
use crate::types::{ColumnType, Store};

/// A spec validation or load failure, always naming the offending entity/file.
#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    /// The `entities/` root could not be read.
    // A field named `source` IS the error source to thiserror, so these
    // messages name only what was attempted and the offending path; the cause
    // comes from `.source()` and must not be interpolated as well.
    #[error("reading entities root {path:?}")]
    ReadRoot {
        path: PathBuf,
        source: std::io::Error,
    },
    /// An `entity.yaml` could not be read.
    #[error("reading {path:?}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },
    /// An `entity.yaml` did not parse.
    #[error("parsing {path:?}")]
    Parse {
        path: PathBuf,
        source: serde_norway::Error,
    },
    /// An entity failed a validation rule.
    #[error("{entity}: {message}")]
    Invalid { entity: String, message: String },
    /// Two entities claim the same store-qualified relation.
    #[error("duplicate relation {qualified:?} declared by {first} and {second}")]
    DuplicateRelation {
        qualified: String,
        first: String,
        second: String,
    },
}

/// The validated set of entities, keyed for lookup by name and by store-qualified
/// relation.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    entities: Vec<Entity>,
}

impl Catalog {
    /// Build a catalog from already-parsed entities, validating each and checking
    /// for duplicate store-qualified relations across the set.
    pub fn from_entities(entities: Vec<Entity>) -> Result<Self, SpecError> {
        for entity in &entities {
            validate(entity)?;
        }
        // No two entities may own the same store-qualified relation.
        let mut seen: BTreeMap<String, &str> = BTreeMap::new();
        for entity in &entities {
            for &store in &entity.generate.stores {
                let qualified = entity.qualified_name(store);
                if let Some(first) = seen.insert(qualified.clone(), &entity.name) {
                    return Err(SpecError::DuplicateRelation {
                        qualified,
                        first: first.to_string(),
                        second: entity.name.clone(),
                    });
                }
            }
        }
        Ok(Self { entities })
    }

    /// Load every `<root>/*/entity.yaml`, parse and validate it, and build the
    /// catalog. Entities are ordered by directory name for a stable generated diff.
    pub fn load_dir(root: impl AsRef<Path>) -> Result<Self, SpecError> {
        let root = root.as_ref();
        let mut dirs: Vec<PathBuf> = fs::read_dir(root)
            .map_err(|source| SpecError::ReadRoot {
                path: root.to_path_buf(),
                source,
            })?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();

        let mut entities = Vec::new();
        for dir in dirs {
            let file = dir.join("entity.yaml");
            if !file.exists() {
                continue;
            }
            let text = fs::read_to_string(&file).map_err(|source| SpecError::ReadFile {
                path: file.clone(),
                source,
            })?;
            let entity: Entity =
                serde_norway::from_str(&text).map_err(|source| SpecError::Parse {
                    path: file.clone(),
                    source,
                })?;
            entities.push(entity);
        }
        Self::from_entities(entities)
    }

    /// Every entity, in load order.
    #[must_use]
    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    /// Find an entity by name.
    #[must_use]
    pub fn by_name(&self, name: &str) -> Option<&Entity> {
        self.entities.iter().find(|e| e.name == name)
    }

    /// Find the entity that owns a store-qualified relation (`lake.main.golden_records`).
    #[must_use]
    pub fn by_qualified_name(&self, store: Store, relation: &str) -> Option<&Entity> {
        self.entities
            .iter()
            .find(|e| e.has_store(store) && e.relation == relation)
    }

    /// Every entity generated for the given store, in load order.
    #[must_use]
    pub fn for_store(&self, store: Store) -> Vec<&Entity> {
        self.entities
            .iter()
            .filter(|e| e.has_store(store))
            .collect()
    }
}

fn invalid(entity: &Entity, message: impl Into<String>) -> SpecError {
    SpecError::Invalid {
        entity: entity.name.clone(),
        message: message.into(),
    }
}

/// Validate one entity against the spec rules. Each failure names a cause.
fn validate(entity: &Entity) -> Result<(), SpecError> {
    if entity.kind != "entity" {
        return Err(invalid(
            entity,
            format!("kind must be \"entity\", got {:?}", entity.kind),
        ));
    }
    if entity.description.trim().is_empty() {
        return Err(invalid(entity, "a table description is required"));
    }
    if entity.generate.stores.is_empty() {
        return Err(invalid(
            entity,
            "generate.stores must list at least one store",
        ));
    }
    if entity.generate.languages.is_empty() {
        return Err(invalid(
            entity,
            "generate.languages must list at least one language",
        ));
    }
    if entity.columns.is_empty() {
        return Err(invalid(
            entity,
            "an entity must declare at least one column",
        ));
    }

    // Column names are unique, and each column carries a description.
    let mut names = std::collections::BTreeSet::new();
    for column in &entity.columns {
        if !names.insert(column.name.as_str()) {
            return Err(invalid(
                entity,
                format!("duplicate column {:?}", column.name),
            ));
        }
        if column.description.trim().is_empty() {
            return Err(invalid(
                entity,
                format!("column {:?} needs a description", column.name),
            ));
        }
        // `serial` is a bigint-only decoration (it renders bigserial).
        if column.serial && column.col_type != ColumnType::Bigint {
            return Err(invalid(
                entity,
                format!("serial column {:?} must be of type bigint", column.name),
            ));
        }
    }

    // The key is non-empty and every key column exists.
    if entity.key.is_empty() {
        return Err(invalid(entity, "key must name at least one column"));
    }
    for key_col in &entity.key {
        if !names.contains(key_col.as_str()) {
            return Err(invalid(
                entity,
                format!("key column {key_col:?} is not a column"),
            ));
        }
    }

    // Index columns, where present, must be real columns.
    for index in &entity.indexes {
        for col in &index.columns {
            if !names.contains(col.as_str()) {
                return Err(invalid(
                    entity,
                    format!("index column {col:?} is not a column"),
                ));
            }
        }
    }

    Ok(())
}
