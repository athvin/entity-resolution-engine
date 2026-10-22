//! The reconciler: make one live store match the catalog, and say what happened.
//!
//! Declarative, not versioned. There is no migration file: the catalog compiled
//! into the binary is the desired state, the live store's `information_schema`
//! is the actual state, and the plan is computed at every apply point (control
//! plane boot, every lake job, tenant provisioning, the deploy's tenant sweep).
//! What makes it safe is what the plan refuses to contain — only `CREATE TABLE`
//! for an absent relation and `ADD COLUMN` for an absent nullable column are ever
//! applied; a drop, a retype, a tightened constraint is `ERR_SCHEMA_BREAKING`
//! and stops the caller. What makes it auditable is the ledger: every applied
//! statement is a row in the store's own `schema_migrations`, keyed by the
//! catalog version ([`crate::catalog_hash`]) it was reconciling toward, and a
//! `verified` row marks each new catalog version that found the store current.
//! The newest row is the version the store is at; an operator reads the
//! history as a migration log, which is the one thing a versioned tool gives
//! that a bare declarative apply would not.

use std::collections::BTreeSet;

use er_entity_spec::{Catalog, Store};
use tokio_postgres::Client;

use crate::{
    DiffItem, Planned, catalog_hash, catalog_without, plan_migration, postgres_relations,
    read_lake_columns, read_postgres_columns,
};

/// Why a reconciliation stopped. The Display strings never interpolate their
/// source (the workspace's error rule); reporters walk the chain.
#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    /// The live store differs from the catalog in a way the planner refuses to
    /// auto-apply: a dropped or retyped column, a loosened key, a `NOT NULL`
    /// column added without a default. The lake's remedy is a rebuild from
    /// parquet; the control plane's is a hand-written migration, after which the
    /// next boot verifies the result.
    #[error("ERR_SCHEMA_BREAKING: {} difference(s) the reconciler refuses to auto-apply:{}", .0.len(), render(.0))]
    Breaking(Vec<DiffItem>),
    /// A control-plane query or DDL statement failed.
    #[error("reconciling the control database")]
    Postgres(#[from] tokio_postgres::Error),
    /// A lake query or DDL statement failed.
    #[error("reconciling the lake")]
    Duckdb(#[from] duckdb::Error),
    /// The lake catalog could not be narrowed to its DDL-owned relations.
    #[error("narrowing the catalog to the lake's DDL-owned relations")]
    Spec(#[from] er_entity_spec::SpecError),
}

fn render(items: &[DiffItem]) -> String {
    use std::fmt::Write as _;
    items.iter().fold(String::new(), |mut out, d| {
        let _ = write!(out, "\n  {}: {}", d.relation, d.message);
        out
    })
}

/// What one reconciliation did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconciled {
    /// The catalog version the store was reconciled toward.
    pub catalog_hash: String,
    /// The statements applied, in order; empty when the store was current.
    pub applied: Vec<Planned>,
    /// Columns the store carries beyond the catalog (see [`Severity::Ahead`]).
    pub ahead: Vec<DiffItem>,
    /// Whether the ledger already recorded this catalog version before the
    /// call (so nothing was written this time).
    pub ledger_was_current: bool,
}

impl Reconciled {
    /// The one-line status the tenant report and the job log print.
    #[must_use]
    pub fn status(&self) -> &'static str {
        if self.applied.is_empty() {
            "current"
        } else {
            "migrated"
        }
    }

    /// A one-line description: what was applied, and what is ahead.
    #[must_use]
    pub fn detail(&self) -> String {
        let mut parts = Vec::new();
        if self.applied.is_empty() {
            parts.push("nothing pending".to_string());
        } else {
            parts.push(format!(
                "applied {}: {}",
                self.applied.len(),
                self.applied
                    .iter()
                    .map(|p| p.relation.as_str())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.ahead.is_empty() {
            parts.push(format!(
                "ahead: {}",
                self.ahead
                    .iter()
                    .map(|d| format!("{} ({})", d.relation, d.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        parts.join("; ")
    }
}

/// The ledger `kind` a reconciliation writes: `applied` per statement, else
/// `verified` once per new catalog version.
#[must_use]
pub fn ledger_kind(applied: bool) -> &'static str {
    if applied { "applied" } else { "verified" }
}

/// Reconcile the control database (`public.*`) to the catalog, under the same
/// advisory lock the boot baseline takes, in one transaction: read the live
/// columns, plan, refuse a breaking plan, apply the safe statements, then write
/// the ledger. `applied_by` names the writer (binary + version) in each row.
///
/// # Errors
/// [`ReconcileError::Breaking`] leaves the database untouched; a Postgres error
/// rolls the transaction back.
pub async fn reconcile_postgres(
    client: &mut Client,
    catalog: &Catalog,
    applied_by: &str,
) -> Result<Reconciled, ReconcileError> {
    let hash = catalog_hash(catalog, Store::Postgres);
    let relations = postgres_relations(catalog);
    let tx = client.transaction().await?;
    tx.execute(
        "SELECT pg_advisory_xact_lock(hashtext('erserver.ensure_schema'), 0)",
        &[],
    )
    .await?;
    let live = read_postgres_columns(tx.client(), &relations).await?;
    let plan = plan_migration(catalog, Store::Postgres, &live);
    if !plan.breaking.is_empty() {
        return Err(ReconcileError::Breaking(plan.breaking));
    }
    for stmt in &plan.statements {
        tx.batch_execute(&stmt.sql).await?;
    }
    // The ledger: one `applied` row per statement, else one `verified` row the
    // first time this catalog version meets an already-current store.
    let latest = er_entities::schema_migration::read::latest(tx.client())
        .await?
        .into_iter()
        .next()
        .map(|row| row.catalog_hash);
    let ledger_was_current = latest.as_deref() == Some(hash.as_str());
    if plan.statements.is_empty() {
        if !ledger_was_current {
            er_entities::schema_migration::create::record(
                tx.client(),
                &er_ids::new_v7(),
                &hash,
                ledger_kind(false),
                None,
                None,
                applied_by,
            )
            .await?;
        }
    } else {
        for stmt in &plan.statements {
            er_entities::schema_migration::create::record(
                tx.client(),
                &er_ids::new_v7(),
                &hash,
                ledger_kind(true),
                Some(&stmt.relation),
                Some(&stmt.sql),
                applied_by,
            )
            .await?;
        }
    }
    tx.commit().await?;
    Ok(Reconciled {
        catalog_hash: hash,
        applied: plan.statements,
        ahead: plan.ahead,
        ledger_was_current,
    })
}

/// The lake relations the reconciler owns: every `duckdb` entity's relation
/// except the stage-materialised ones (`er_schema::model::stage_owned`), which
/// each run rebuilds whole.
#[must_use]
pub fn lake_relations(catalog: &Catalog) -> Vec<String> {
    let stage: BTreeSet<String> = er_schema::model::stage_owned().into_iter().collect();
    catalog
        .for_store(Store::Duckdb)
        .into_iter()
        .filter(|e| !stage.contains(&e.relation))
        .map(|e| e.relation.clone())
        .collect()
}

/// Reconcile an attached tenant lake (`lake.main.*`) to the catalog: read the
/// live columns of the DDL-owned relations, plan, refuse a breaking plan, apply
/// the safe statements in one transaction, then write the lake's own ledger with
/// the snapshot the apply produced. The caller holds whatever serialises writers
/// to this lake (the job queue's one-active-job-per-org rule); the reconciler
/// takes no lock of its own.
///
/// # Errors
/// [`ReconcileError::Breaking`] leaves the lake untouched; a DuckDB error on
/// apply rolls the transaction back.
pub fn reconcile_lake(
    conn: &duckdb::Connection,
    catalog: &Catalog,
    applied_by: &str,
) -> Result<Reconciled, ReconcileError> {
    let stage: BTreeSet<String> = er_schema::model::stage_owned().into_iter().collect();
    let owned = catalog_without(catalog, Store::Duckdb, &stage)?;
    let hash = catalog_hash(catalog, Store::Duckdb);
    let relations: Vec<String> = owned
        .for_store(Store::Duckdb)
        .into_iter()
        .map(|e| e.relation.clone())
        .collect();
    let live = read_lake_columns(conn, &relations)?;
    let plan = plan_migration(&owned, Store::Duckdb, &live);
    if !plan.breaking.is_empty() {
        return Err(ReconcileError::Breaking(plan.breaking));
    }
    if !plan.statements.is_empty() {
        conn.execute_batch("BEGIN;")?;
        let applied: duckdb::Result<()> = plan
            .statements
            .iter()
            .try_for_each(|stmt| conn.execute_batch(&stmt.sql));
        if let Err(e) = applied {
            // Best effort: the error that matters is the apply's.
            let _ = conn.execute_batch("ROLLBACK;");
            return Err(e.into());
        }
        conn.execute_batch("COMMIT;")?;
    }
    let snapshot = latest_snapshot(conn)?;
    let latest = er_entities::lake_schema_migration::read::latest(conn)?
        .into_iter()
        .next()
        .map(|row| row.catalog_hash);
    let ledger_was_current = latest.as_deref() == Some(hash.as_str());
    let now = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.6f")
        .to_string();
    if plan.statements.is_empty() {
        if !ledger_was_current {
            er_entities::lake_schema_migration::create::record(
                conn,
                &er_ids::new_v7(),
                &now,
                &hash,
                ledger_kind(false),
                None,
                None,
                applied_by,
                snapshot,
            )?;
        }
    } else {
        for stmt in &plan.statements {
            er_entities::lake_schema_migration::create::record(
                conn,
                &er_ids::new_v7(),
                &now,
                &hash,
                ledger_kind(true),
                Some(&stmt.relation),
                Some(&stmt.sql),
                applied_by,
                snapshot,
            )?;
        }
    }
    Ok(Reconciled {
        catalog_hash: hash,
        applied: plan.statements,
        ahead: plan.ahead,
        ledger_was_current,
    })
}

/// The attached lake's newest snapshot id (`None` before its first commit).
fn latest_snapshot(conn: &duckdb::Connection) -> duckdb::Result<Option<i64>> {
    conn.query_row(
        "SELECT max(snapshot_id) FROM ducklake_snapshots('lake')",
        [],
        |r| r.get::<_, Option<i64>>(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Severity;

    #[test]
    fn breaking_renders_every_difference() {
        let err = ReconcileError::Breaking(vec![
            DiffItem {
                relation: "public.jobs".to_string(),
                severity: Severity::Breaking,
                message: "column priority: type integer -> bigint".to_string(),
            },
            DiffItem {
                relation: "public.orgs".to_string(),
                severity: Severity::Breaking,
                message: "column env: nullable -> NOT NULL".to_string(),
            },
        ]);
        let text = err.to_string();
        assert!(
            text.starts_with("ERR_SCHEMA_BREAKING: 2 difference(s)"),
            "{text}"
        );
        assert!(text.contains("public.jobs: column priority"));
        assert!(text.contains("public.orgs: column env"));
    }

    #[test]
    fn detail_names_what_was_applied_and_what_is_ahead() {
        let r = Reconciled {
            catalog_hash: "abc".to_string(),
            applied: vec![
                Planned {
                    relation: "public.jobs".to_string(),
                    sql: "ALTER TABLE public.jobs ADD COLUMN x text;".to_string(),
                },
                Planned {
                    relation: "public.jobs".to_string(),
                    sql: "ALTER TABLE public.jobs ADD COLUMN y text;".to_string(),
                },
            ],
            ahead: vec![DiffItem {
                relation: "public.orgs".to_string(),
                severity: Severity::Ahead,
                message: "column legacy is in the live store but not declared".to_string(),
            }],
            ledger_was_current: false,
        };
        assert_eq!(r.status(), "migrated");
        assert_eq!(
            r.detail(),
            "applied 2: public.jobs; ahead: public.orgs (column legacy is in the live store but not declared)"
        );
        let current = Reconciled {
            applied: Vec::new(),
            ahead: Vec::new(),
            ..r
        };
        assert_eq!(current.status(), "current");
        assert_eq!(current.detail(), "nothing pending");
    }

    #[test]
    fn ledger_kinds() {
        assert_eq!(ledger_kind(true), "applied");
        assert_eq!(ledger_kind(false), "verified");
    }
}
