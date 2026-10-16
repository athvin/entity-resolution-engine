//! `er-entity-diff` — does the live store match the entity catalog?
//!
//! The desired state is the catalog; the actual state is read from the live store
//! into the same `(name, type, nullable)` shape (Postgres via `information_schema`,
//! DuckLake via `duckdb`'s `information_schema`), and the comparison is column by
//! column in declaration order. Each difference is classified safe (a plan can
//! auto-apply it) or breaking (it needs an explicit, human-written migration).

mod postgres;

pub use postgres::read_postgres_columns;

use er_entity_spec::{Catalog, Entity, Store};

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

    // Columns in the live store the entity no longer declares: dropping is
    // breaking (and the differ never auto-drops).
    let declared: std::collections::BTreeSet<&str> =
        entity.columns.iter().map(|c| c.name.as_str()).collect();
    for live in live_cols {
        if !declared.contains(live.name.as_str()) {
            diffs.push(DiffItem {
                relation: qualify(&entity.relation),
                severity: Severity::Breaking,
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
            ("job_id", "text", false),
            ("org", "text", false),
            ("kind", "text", false),
            ("params", "jsonb", false),
            ("state", "text", false),
            ("priority", "bigint", false),
            ("idempotency_key", "text", true),
            ("run_id", "text", true),
            ("attempt", "bigint", false),
            ("max_attempts", "bigint", false),
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
    fn a_missing_relation_is_a_safe_create() {
        let diffs = diff_store(&catalog(), Store::Postgres, &BTreeMap::new());
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].severity, Severity::Safe);
        assert!(diffs[0].message.contains("create table"));
    }
}
