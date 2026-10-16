//! The migration-plan writer. A safe difference (a new table, a new nullable
//! column) becomes a statement the plan applies; a breaking one (a drop, a
//! retype, a loosened key) is refused — the operator writes that migration by
//! hand and the differ verifies the result. `migrate plan` auto-generates safe
//! operations only.

use std::collections::BTreeMap;

use er_entity_spec::{Catalog, Column, Entity, Store};

use crate::{DiffItem, LiveColumn, Severity, diff_store};

/// A migration plan for one store.
#[derive(Debug, Clone, Default)]
pub struct MigrationPlan {
    /// Statements that reconcile the live store to the catalog, in order.
    pub statements: Vec<String>,
    /// Differences that cannot be auto-applied; each needs a hand-written migration.
    pub breaking: Vec<DiffItem>,
}

impl MigrationPlan {
    /// Whether the plan has nothing to do and nothing it refuses.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.statements.is_empty() && self.breaking.is_empty()
    }
}

/// Order entities so a referenced relation precedes the relation that references
/// it (a depth-first topological order over column `references`), so CREATE TABLE
/// statements apply without a dangling foreign key. Cycles (none in practice) fall
/// back to first-visit order.
fn topo_order<'a>(entities: &[&'a Entity]) -> Vec<&'a Entity> {
    let by_rel: BTreeMap<&str, &'a Entity> =
        entities.iter().map(|e| (e.relation.as_str(), *e)).collect();
    let mut visited = std::collections::BTreeSet::new();
    let mut ordered = Vec::new();

    fn visit<'a>(
        entity: &'a Entity,
        by_rel: &BTreeMap<&str, &'a Entity>,
        visited: &mut std::collections::BTreeSet<String>,
        ordered: &mut Vec<&'a Entity>,
    ) {
        if !visited.insert(entity.relation.clone()) {
            return;
        }
        for col in &entity.columns {
            if let Some(reference) = &col.references {
                if let Some(dep) = by_rel.get(reference.relation.as_str()) {
                    visit(dep, by_rel, visited, ordered);
                }
            }
        }
        ordered.push(entity);
    }

    for entity in entities {
        visit(entity, &by_rel, &mut visited, &mut ordered);
    }
    ordered
}

/// A nullable column's `ADD COLUMN` fragment for the given store.
fn add_column_sql(store: Store, relation: &str, column: &Column) -> String {
    match store {
        Store::Duckdb => format!(
            "ALTER TABLE lake.main.{relation} ADD COLUMN {} {};",
            column.name,
            column.col_type.ducklake_type()
        ),
        Store::Postgres => {
            let mut sql = format!(
                "ALTER TABLE public.{relation} ADD COLUMN {} {}",
                column.name,
                column.col_type.postgres_type()
            );
            if let Some(default) = &column.default {
                sql.push_str(" DEFAULT ");
                sql.push_str(default);
            }
            sql.push(';');
            sql
        }
    }
}

/// The `CREATE TABLE` statement for an entity in the given store.
fn create_table_sql(store: Store, entity: &Entity) -> String {
    match store {
        Store::Duckdb => er_entity_gen::emit_lake_ddl(entity)
            .map(|s| format!("{s};"))
            .unwrap_or_default(),
        Store::Postgres => er_entity_gen::emit_postgres_ddl(entity).unwrap_or_default(),
    }
}

/// Build a migration plan: the safe statements that reconcile the live store to
/// the catalog, plus the breaking differences it refuses to auto-apply.
#[must_use]
pub fn plan_migration(
    catalog: &Catalog,
    store: Store,
    live: &BTreeMap<String, Vec<LiveColumn>>,
) -> MigrationPlan {
    // Breaking differences come straight from the differ.
    let breaking = diff_store(catalog, store, live)
        .into_iter()
        .filter(|d| d.severity == Severity::Breaking)
        .collect();
    let mut plan = MigrationPlan {
        statements: Vec::new(),
        breaking,
    };

    // Safe operations are re-derived from the catalog so each statement is real
    // DDL (not a diff message): a missing relation -> CREATE TABLE; a missing
    // nullable column -> ADD COLUMN. CREATEs are ordered so a referenced table is
    // created before the table that references it (the FK is valid on apply).
    for entity in topo_order(&catalog.for_store(store)) {
        match live.get(&entity.relation) {
            None => plan.statements.push(create_table_sql(store, entity)),
            Some(cols) => {
                let present: std::collections::BTreeSet<&str> =
                    cols.iter().map(|c| c.name.as_str()).collect();
                for column in &entity.columns {
                    if !present.contains(column.name.as_str()) && column.nullable {
                        plan.statements
                            .push(add_column_sql(store, &entity.relation, column));
                    }
                }
            }
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    const JOB_YAML: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../entities/job/entity.yaml"
    ));

    fn catalog() -> Catalog {
        Catalog::from_entities(vec![serde_norway::from_str(JOB_YAML).unwrap()]).unwrap()
    }

    #[test]
    fn an_absent_relation_plans_a_create_table() {
        let plan = plan_migration(&catalog(), Store::Postgres, &BTreeMap::new());
        assert_eq!(plan.statements.len(), 1);
        assert!(plan.statements[0].contains("CREATE TABLE IF NOT EXISTS public.jobs"));
        assert_eq!(plan.breaking, Vec::new());
    }

    #[test]
    fn a_missing_nullable_column_plans_an_add_column_and_a_type_change_is_refused() {
        // Live `jobs` missing the nullable `run_id`, and with `priority` as integer.
        let cols: Vec<LiveColumn> = [
            ("job_id", "text", false),
            ("org", "text", false),
            ("kind", "text", false),
            ("params", "jsonb", false),
            ("state", "text", false),
            ("priority", "integer", false),
            ("idempotency_key", "text", true),
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
        .collect();
        let mut live = BTreeMap::new();
        live.insert("jobs".to_string(), cols);

        let plan = plan_migration(&catalog(), Store::Postgres, &live);
        // run_id is a missing nullable column -> a safe ADD COLUMN.
        assert!(
            plan.statements
                .iter()
                .any(|s| s.contains("ADD COLUMN run_id text")),
            "{:?}",
            plan.statements
        );
        // priority integer -> bigint is breaking and refused.
        assert!(
            plan.breaking
                .iter()
                .any(|d| d.message.contains("priority: type integer -> bigint"))
        );
    }
}
