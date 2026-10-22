//! `er-entity-diff` — does the live store match the entity catalog?
//!
//! The desired state is the catalog; the actual state is read from the live store
//! into the same `(name, type, nullable)` shape (Postgres via `information_schema`,
//! DuckLake via `duckdb`'s `information_schema`), and the comparison is column by
//! column in declaration order. Each difference is classified safe (a plan can
//! auto-apply it) or breaking (it needs an explicit, human-written migration).

mod lake;
mod migrate;
mod postgres;
mod reconcile;

pub use lake::{attach_lake, read_lake_columns};
pub use migrate::{MigrationPlan, Planned, plan_migration};
pub use postgres::read_postgres_columns;
pub use reconcile::{
    ReconcileError, Reconciled, lake_relations, ledger_kind, reconcile_lake, reconcile_postgres,
};

use er_entity_spec::{Catalog, Entity, SpecError, Store};
use sha2::{Digest, Sha256};

/// One column as the live store reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveColumn {
    /// The column name.
    pub name: String,
    /// The store's type, normalised to the entity vocabulary's spelling
    /// (`timestamp with time zone` -> `timestamptz`, etc.).
    pub type_name: String,
    /// Whether the column admits NULL.
    pub nullable: bool,
}

/// How severe a difference is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// A plan can auto-apply it (add a nullable column, create a table).
    Safe,
    /// It needs an explicit migration (drop/rename/retype/loosen a key).
    Breaking,
    /// The live store carries a column the catalog does not declare: the store
    /// is AHEAD of this catalog (an image rollback across an additive change,
    /// or an operator's extra column). Never dropped, never refused — every
    /// statement names its columns (E011), so an old binary serves an ahead
    /// store correctly. Reported so an operator can see it.
    Ahead,
}

/// One catalog-vs-live difference for a relation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffItem {
    /// The store-qualified relation (`public.jobs`).
    pub relation: String,
    /// The severity.
    pub severity: Severity,
    /// A one-line `live -> declared` explanation.
    pub message: String,
}

/// The canonical type a Postgres entity column should have in the live store.
fn expected_pg_type(col: &er_entity_spec::Column) -> &'static str {
    col.col_type.postgres_type()
}

/// The catalog's schema version for one store: the sha256 of the generated DDL
/// of every entity in that store, in relation order. Two commits whose entities
/// render the same DDL have the same version; any column, type, index or
/// constraint change moves it. This is what the `schema_migrations` ledgers
/// record, so "behind" and "ahead" are a comparison of hashes rather than of
/// file names, and a store carries no sequence number that two branches could
/// both claim.
#[must_use]
pub fn catalog_hash(catalog: &Catalog, store: Store) -> String {
    let mut entities = catalog.for_store(store);
    entities.sort_by(|a, b| a.relation.cmp(&b.relation));
    let mut hasher = Sha256::new();
    for entity in entities {
        let ddl = match store {
            Store::Duckdb => er_entity_gen::emit_lake_ddl(entity).unwrap_or_default(),
            Store::Postgres => er_entity_gen::emit_postgres_ddl(entity).unwrap_or_default(),
        };
        hasher.update(entity.relation.as_bytes());
        hasher.update(b"\0");
        hasher.update(ddl.as_bytes());
        hasher.update(b"\0");
    }
    hex::encode(hasher.finalize())
}

/// The catalog restricted to one store, minus the relations in `skip`. The lake
/// reconciler uses it to leave stage-materialised relations alone: a `Stage`
/// relation is rebuilt whole by `CREATE OR REPLACE TABLE ... AS SELECT` on every
/// run, so its live shape is whatever the last run's engine produced (no `NOT
/// NULL`, the previous column set) and the next run corrects it; only `Ddl`
/// relations are created up front and need `ADD COLUMN`.
///
/// # Errors
/// The filtered set fails `Catalog::from_entities` (a reference to a skipped
/// relation — none in practice, since lake entities carry no references).
pub fn catalog_without(
    catalog: &Catalog,
    store: Store,
    skip: &std::collections::BTreeSet<String>,
) -> Result<Catalog, SpecError> {
    Catalog::from_entities(
        catalog
            .for_store(store)
            .into_iter()
            .filter(|e| !skip.contains(&e.relation))
            .cloned()
            .collect(),
    )
}

/// The Postgres relations a live read should cover for this catalog: every
/// declared relation plus the `<relation>_history` companion of each
/// `history: true` entity (generated DDL the differ checks for existence).
#[must_use]
pub fn postgres_relations(catalog: &Catalog) -> Vec<String> {
    let mut relations = Vec::new();
    for entity in catalog.for_store(Store::Postgres) {
        relations.push(entity.relation.clone());
        if entity.generate.history {
            relations.push(format!("{}_history", entity.relation));
        }
    }
    relations
}

/// Compare one store's catalog entities to the live columns read from that store.
/// `live` maps a bare relation name to its columns in ordinal order.
#[must_use]
pub fn diff_store(
    catalog: &Catalog,
    store: Store,
    live: &std::collections::BTreeMap<String, Vec<LiveColumn>>,
) -> Vec<DiffItem> {
    let qualify = |rel: &str| format!("{}.{rel}", store.qualifier());
    let mut diffs = Vec::new();

    for entity in catalog.for_store(store) {
        let Some(live_cols) = live.get(&entity.relation) else {
            diffs.push(DiffItem {
                relation: qualify(&entity.relation),
                severity: Severity::Safe,
                message: format!("relation {} is absent; create table", entity.relation),
            });
            continue;
        };
        diff_entity(entity, store, live_cols, &qualify, &mut diffs);
    }

    // History companions are generated DDL, not entities: the differ checks
    // existence only (shape drift is the boot replay's business, like indexes).
    if store == Store::Postgres {
        for entity in catalog.for_store(store) {
            if !entity.generate.history {
                continue;
            }
            let companion = format!("{}_history", entity.relation);
            if !live.contains_key(&companion) {
                diffs.push(DiffItem {
                    relation: qualify(&companion),
                    severity: Severity::Safe,
                    message: format!(
                        "history companion {companion} is absent; create companion + trigger"
                    ),
                });
            }
        }
    }
    diffs
}

fn diff_entity(
    entity: &Entity,
    store: Store,
    live_cols: &[LiveColumn],
    qualify: &impl Fn(&str) -> String,
    diffs: &mut Vec<DiffItem>,
) {
    let live_by_name: std::collections::BTreeMap<&str, &LiveColumn> =
        live_cols.iter().map(|c| (c.name.as_str(), c)).collect();

    for col in &entity.columns {
        let Some(live) = live_by_name.get(col.name.as_str()) else {
            // A declared column missing from the live store: adding a nullable
            // column is safe; a NOT NULL add needs a default (breaking here).
            let severity = if col.nullable {
                Severity::Safe
            } else {
                Severity::Breaking
            };
            diffs.push(DiffItem {
                relation: qualify(&entity.relation),
                severity,
                message: format!("column {} is absent; add column", col.name),
            });
            continue;
        };
        let expected_type = match store {
            Store::Postgres => expected_pg_type(col),
            Store::Duckdb => col.col_type.ducklake_type(),
        };
        if live.type_name != expected_type {
            diffs.push(DiffItem {
                relation: qualify(&entity.relation),
                severity: Severity::Breaking,
                message: format!(
                    "column {}: type {} -> {}",
                    col.name, live.type_name, expected_type
                ),
            });
        }
        // The live store allowing NULL where the entity declares NOT NULL is a
        // loosened constraint (breaking); the reverse is a tightening the differ
        // does not force here.
        if live.nullable && !col.nullable {
            diffs.push(DiffItem {
                relation: qualify(&entity.relation),
                severity: Severity::Breaking,
                message: format!("column {}: nullable -> NOT NULL", col.name),
            });
        }
    }

    // Columns in the live store the entity does not declare: the store is ahead
    // of this catalog. The differ never drops, and a plan never refuses on it —
    // an additive release rolled back must keep serving (every statement names
    // its columns, E011) — but it is reported so an operator sees it.
    let declared: std::collections::BTreeSet<&str> =
        entity.columns.iter().map(|c| c.name.as_str()).collect();
    for live in live_cols {
        if !declared.contains(live.name.as_str()) {
            diffs.push(DiffItem {
                relation: qualify(&entity.relation),
                severity: Severity::Ahead,
                message: format!("column {} is in the live store but not declared", live.name),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const JOB_YAML: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../entities/job/entity.yaml"
    ));

    fn catalog() -> Catalog {
        Catalog::from_entities(vec![serde_norway::from_str(JOB_YAML).unwrap()]).unwrap()
    }

    fn live_job() -> Vec<LiveColumn> {
        // The live `jobs` row as the current ensure_schema creates it (job columns
        // widened to bigint already).
        [
            ("id", "text", false),
            ("org_id", "text", false),
            ("kind", "text", false),
            ("params", "jsonb", false),
            ("state", "text", false),
            ("priority", "bigint", false),
            ("idempotency_key", "text", true),
            ("run_id", "text", true),
            ("attempt", "bigint", false),
            ("max_attempts", "bigint", false),
            ("error_class", "text", true),
            ("error_detail", "text", true),
            ("not_before", "timestamptz", true),
            ("created_by", "text", true),
            ("schedule_id", "text", true),
            ("resource_class", "text", true),
            ("trace_context", "text", true),
            ("created_at", "timestamptz", false),
            ("updated_at", "timestamptz", false),
            ("started_at", "timestamptz", true),
            ("finished_at", "timestamptz", true),
        ]
        .into_iter()
        .map(|(n, t, nl)| LiveColumn {
            name: n.to_string(),
            type_name: t.to_string(),
            nullable: nl,
        })
        .collect()
    }

    #[test]
    fn a_matching_schema_has_no_diff() {
        let mut live = BTreeMap::new();
        live.insert("jobs".to_string(), live_job());
        assert_eq!(diff_store(&catalog(), Store::Postgres, &live), Vec::new());
    }

    #[test]
    fn an_integer_column_is_a_breaking_type_diff() {
        let mut cols = live_job();
        cols.iter_mut()
            .find(|c| c.name == "priority")
            .unwrap()
            .type_name = "integer".to_string();
        let mut live = BTreeMap::new();
        live.insert("jobs".to_string(), cols);
        let diffs = diff_store(&catalog(), Store::Postgres, &live);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].severity, Severity::Breaking);
        assert!(
            diffs[0]
                .message
                .contains("priority: type integer -> bigint")
        );
    }

    #[test]
    fn an_undeclared_live_column_means_the_store_is_ahead_not_broken() {
        let mut cols = live_job();
        cols.push(LiveColumn {
            name: "legacy_note".to_string(),
            type_name: "text".to_string(),
            nullable: true,
        });
        let mut live = BTreeMap::new();
        live.insert("jobs".to_string(), cols);
        let diffs = diff_store(&catalog(), Store::Postgres, &live);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].severity, Severity::Ahead);
        // A plan tolerates it: nothing to apply, nothing refused, reported.
        let plan = plan_migration(&catalog(), Store::Postgres, &live);
        assert!(plan.is_empty(), "{plan:?}");
        assert_eq!(plan.ahead.len(), 1);
    }

    #[test]
    fn the_catalog_hash_is_stable_and_moves_with_the_ddl() {
        let a = catalog_hash(&catalog(), Store::Postgres);
        let b = catalog_hash(&catalog(), Store::Postgres);
        assert_eq!(a, b);
        assert_eq!(a.len(), 64, "sha256 hex");
        // A different catalog (one more nullable column) is a different version.
        let mut entity: Entity = serde_norway::from_str(JOB_YAML).unwrap();
        entity.columns.push(er_entity_spec::Column {
            name: "extra".to_string(),
            ..entity.columns.last().unwrap().clone()
        });
        let changed = Catalog::from_entities(vec![entity]).unwrap();
        assert_ne!(a, catalog_hash(&changed, Store::Postgres));
        // The lake version of a Postgres-only catalog hashes nothing, and is
        // still a deterministic value.
        assert_eq!(
            catalog_hash(&catalog(), Store::Duckdb),
            catalog_hash(&catalog(), Store::Duckdb)
        );
    }

    #[test]
    fn the_lake_reconciler_leaves_stage_relations_alone() {
        let full = er_entities::catalog::catalog();
        let owned = lake_relations(&full);
        let stage = er_schema::model::stage_owned();
        for s in &stage {
            assert!(!owned.contains(s), "{s} is stage-materialised");
        }
        assert!(owned.contains(&"raw_records".to_string()));
        assert!(owned.contains(&"schema_migrations".to_string()));
        // And the narrowed catalog has exactly those relations.
        let skip: std::collections::BTreeSet<String> = stage.into_iter().collect();
        let narrowed = catalog_without(&full, Store::Duckdb, &skip).unwrap();
        let mut names: Vec<String> = narrowed
            .for_store(Store::Duckdb)
            .into_iter()
            .map(|e| e.relation.clone())
            .collect();
        names.sort();
        let mut expected = owned;
        expected.sort();
        assert_eq!(names, expected);
    }

    #[test]
    fn an_absent_history_companion_is_a_safe_diff() {
        let webhook: Entity = serde_norway::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../entities/webhook/entity.yaml"
        )))
        .unwrap();
        let live_cols: Vec<LiveColumn> = webhook
            .columns
            .iter()
            .map(|c| LiveColumn {
                name: c.name.clone(),
                type_name: c.col_type.postgres_type().to_string(),
                nullable: c.nullable,
            })
            .collect();
        let catalog = Catalog::from_entities(vec![webhook]).unwrap();
        let mut live = BTreeMap::new();
        live.insert("webhooks".to_string(), live_cols);

        let diffs = diff_store(&catalog, Store::Postgres, &live);
        assert_eq!(diffs.len(), 1, "{diffs:?}");
        assert_eq!(diffs[0].severity, Severity::Safe);
        assert!(diffs[0].message.contains("webhooks_history is absent"));

        live.insert("webhooks_history".to_string(), Vec::new());
        assert_eq!(diff_store(&catalog, Store::Postgres, &live), Vec::new());
    }

    #[test]
    fn a_missing_relation_is_a_safe_create() {
        let diffs = diff_store(&catalog(), Store::Postgres, &BTreeMap::new());
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].severity, Severity::Safe);
        assert!(diffs[0].message.contains("create table"));
    }
}
