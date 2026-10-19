//! The [`Catalog`]: the validated set of entities, loaded from an `entities/`
//! tree. Loading is where every deliberate, loud default lives — a missing
//! `generate` block, a key column that is not a column, a control-only feature on
//! a lake entity each fail here with a named cause, never a silent guess.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::entity::Entity;
use crate::types::{ColumnType, Language, Store};

/// A spec validation or load failure, always naming the offending entity/file.
#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    /// The `entities/` root could not be read.
    #[error("reading entities root {path:?}: {source}")]
    ReadRoot {
        path: PathBuf,
        source: std::io::Error,
    },
    /// An `entity.yaml` could not be read.
    #[error("reading {path:?}: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },
    /// An `entity.yaml` did not parse.
    #[error("parsing {path:?}: {source}")]
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

    /// V9 — every reference points at a declared Postgres relation and one of
    /// its real columns (references were previously taken on faith). A property
    /// of the *whole* tree: [`Catalog::load_dir`] enforces it; a programmatic
    /// subset catalog (the differ's tests, a focused tool) may skip it.
    pub fn validate_references(&self) -> Result<(), SpecError> {
        for entity in &self.entities {
            for column in &entity.columns {
                let Some(reference) = &column.references else {
                    continue;
                };
                let Some(target) = self
                    .entities
                    .iter()
                    .find(|e| e.relation == reference.relation && e.has_store(Store::Postgres))
                else {
                    return Err(invalid(
                        entity,
                        format!(
                            "column {:?} references {}({}), but no Postgres entity declares \
                             that relation",
                            column.name, reference.relation, reference.column
                        ),
                    ));
                };
                if target.column(&reference.column).is_none() {
                    return Err(invalid(
                        entity,
                        format!(
                            "column {:?} references {}({}), but the target declares no such \
                             column",
                            column.name, reference.relation, reference.column
                        ),
                    ));
                }
            }
        }
        Ok(())
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
        let catalog = Self::from_entities(entities)?;
        catalog.validate_references()?;
        Ok(catalog)
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

    // A wire implies rust on both ends; a json or arrow wire with no rust reader is
    // a shape no one can read.
    let has_wire = !entity.generate.wire.is_empty();
    if has_wire && !entity.has_language(Language::Rust) {
        return Err(invalid(
            entity,
            "a wire (arrow/json) requires generate.languages to include rust",
        ));
    }
    // (The browser-facing rule — a read-API entity that returns through the data
    // lane must emit typescript — is enforced in the generator, where the set of
    // read statements that return this entity is known. Arrow + typescript without
    // json is a valid data-lane-only entity, so it is not an error here.)

    // DuckLake enforces nothing but NOT NULL: a lake entity may not carry a
    // default, a reference, accepted_values or an index.
    if entity.has_store(Store::Duckdb) {
        for column in &entity.columns {
            if column.default.is_some()
                || column.references.is_some()
                || column.accepted_values.is_some()
                || column.serial
            {
                return Err(invalid(
                    entity,
                    format!(
                        "column {:?} carries a control-plane feature (default/references/accepted_values/serial) but the entity is on the lake, which enforces only NOT NULL",
                        column.name
                    ),
                ));
            }
        }
        if !entity.indexes.is_empty() {
            return Err(invalid(
                entity,
                "indexes are a control-plane feature; the lake has none",
            ));
        }
    }

    // References and index columns, where present, must be real columns.
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

    // V10 — a natural_key, where declared, is well-formed: real NOT NULL
    // columns, no duplicates, and never the universal id/timestamps (those are
    // row identity and bookkeeping, not what makes two rows the same thing).
    let mut nk_seen = std::collections::BTreeSet::new();
    for nk_col in &entity.natural_key {
        if !nk_seen.insert(nk_col.as_str()) {
            return Err(invalid(
                entity,
                format!("natural_key repeats column {nk_col:?}"),
            ));
        }
        let Some(column) = entity.column(nk_col) else {
            return Err(invalid(
                entity,
                format!("natural_key column {nk_col:?} is not a column"),
            ));
        };
        if column.nullable {
            return Err(invalid(
                entity,
                format!("natural_key column {nk_col:?} must be NOT NULL"),
            ));
        }
        if matches!(nk_col.as_str(), "id" | "created_at" | "updated_at") {
            return Err(invalid(
                entity,
                format!(
                    "natural_key may not include {nk_col:?} — the universal columns are row \
                     identity and bookkeeping, not natural identity"
                ),
            ));
        }
    }

    // The identity conventions (V1–V8). `identity: legacy` marks a
    // not-yet-migrated entity and skips exactly this block — the ratchet.
    if !entity.is_legacy() {
        validate_identity(entity)?;
    }

    Ok(())
}

/// V1–V8: the identity conventions for a migrated (non-legacy) entity.
fn validate_identity(entity: &Entity) -> Result<(), SpecError> {
    // V1 — a NOT NULL `id` column exists: varchar (UUIDv7 or sha256 text), or
    // bigint+serial for database-minted ledgers (audit_log, ddl_log).
    let Some(id) = entity.column("id") else {
        return Err(invalid(entity, "V1: an entity must declare an id column"));
    };
    if id.nullable {
        return Err(invalid(entity, "V1: id must be NOT NULL"));
    }
    let id_ok =
        id.col_type == ColumnType::Varchar || (id.col_type == ColumnType::Bigint && id.serial);
    if !id_ok {
        return Err(invalid(
            entity,
            "V1: id must be varchar (UUIDv7/sha256 text) or bigint with serial: true \
             (a database-minted ledger)",
        ));
    }

    // V2 — the primary key is exactly [id]; natural identity lives in natural_key.
    if entity.key != ["id"] {
        return Err(invalid(
            entity,
            format!("V2: key must be exactly [id], got {:?}", entity.key),
        ));
    }

    // V3 — id leads the declaration (uniform shape across every entity).
    if entity.columns.first().map(|c| c.name.as_str()) != Some("id") {
        return Err(invalid(entity, "V3: id must be the first declared column"));
    }

    // V4 — created_at and updated_at exist as NOT NULL timestamps.
    for name in ["created_at", "updated_at"] {
        let Some(column) = entity.column(name) else {
            return Err(invalid(
                entity,
                format!("V4: an entity must declare {name} (timestamp, NOT NULL)"),
            ));
        };
        if column.col_type != ColumnType::Timestamp || column.nullable {
            return Err(invalid(
                entity,
                format!("V4: {name} must be a NOT NULL timestamp"),
            ));
        }
        if entity.has_store(Store::Postgres) {
            // V5 — control-plane timestamps are wall-clock: defaulted, volatile.
            if column.default.is_none() || !column.volatile {
                return Err(invalid(
                    entity,
                    format!(
                        "V5: control-plane {name} must carry a default (now()) and be \
                         volatile (wall-clock)"
                    ),
                ));
            }
        } else if column.volatile {
            // V6 — lake timestamps are run-logical, written by statements and
            // part of the determinism contract: never volatile (and the lake
            // default ban already holds).
            return Err(invalid(
                entity,
                format!("V6: lake {name} is run-logical and must not be volatile"),
            ));
        }
    }

    // V7/V8 — FK columns are named for what they point at, and point at id.
    for column in &entity.columns {
        let Some(reference) = &column.references else {
            continue;
        };
        let expected = format!("{}_id", crate::entity::singular(&reference.relation));
        if column.name != expected && !column.name.ends_with(&format!("_{expected}")) {
            return Err(invalid(
                entity,
                format!(
                    "V7: foreign-key column {:?} must be named {expected:?} (or \
                     <role>_{expected}) after the relation it references",
                    column.name
                ),
            ));
        }
        if reference.column != "id" {
            return Err(invalid(
                entity,
                format!(
                    "V8: foreign keys reference id, not {}({})",
                    reference.relation, reference.column
                ),
            ));
        }
    }

    Ok(())
}
