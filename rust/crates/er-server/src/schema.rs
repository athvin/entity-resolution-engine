//! Schema reconciliation as the control plane drives it.
//!
//! The reconciler itself lives in `er-entity-diff` (one declarative pass:
//! compare the live store with the catalog compiled into this binary, apply
//! the safe statements, refuse the breaking ones, write the store's own
//! `schema_migrations` ledger). This module is the four places er-server calls
//! it, and the per-tenant bookkeeping around the lake half:
//!
//! - **Boot and `er-server migrate`** reconcile `public.*` ([`crate::pg::ensure_schema`]).
//! - **Every lake job reconciles its lake first** ([`reconcile_lake_env`], from
//!   the dispatcher before any stage runs), under the queue's one-active-job-
//!   per-org rule, which is the writer lock. A breaking difference fails the
//!   job with `ERR_SCHEMA_BREAKING` rather than running the new engine against
//!   an old table.
//! - **Provisioning** reconciles the new lake before the org is promoted to
//!   `active`, so a tenant never starts with a lake whose tables the first run
//!   would have to create.
//! - **The deploy fans out** ([`enqueue_tenant_migrations`] +
//!   [`wait_for_tenant_migrations`], the `er-server migrate-tenants`
//!   subcommand the chart runs as a post-upgrade hook and as a drift CronJob):
//!   one `lake_migrate` job per active org, then a per-tenant report.
//!
//! The control plane caches each lake's position in `tenant_schema_states`
//! ([`record_tenant_state`]) so the report and the drift check never open a
//! lake; the lake's ledger stays authoritative.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::pg;

type PgError = tokio_postgres::Error;

/// What the ledgers record as the writer: this binary and its version.
#[must_use]
pub fn applied_by() -> String {
    format!("er-server {}", env!("CARGO_PKG_VERSION"))
}

/// The lake catalog version this binary carries.
#[must_use]
pub fn lake_catalog_hash() -> String {
    er_entity_diff::catalog_hash(
        &er_entities::catalog::catalog(),
        er_entity_spec::Store::Duckdb,
    )
}

/// Reconcile an org's lake, opened from its env (`ER_LAKE_CATALOG` +
/// `ER_LAKE_DATA_PATH`), to the compiled-in catalog. Blocking: call it from
/// `spawn_blocking`. Returns the job-failure text on any failure, in the
/// dispatcher's convention: "open lake: ..." (transient, retried) or
/// "ERR_SCHEMA_BREAKING: ..." (terminal; a rebuild is needed).
///
/// # Errors
/// A missing env key, a lake that will not open, a breaking difference, or a
/// DDL/ledger failure — each as a message a `FailureClass` can classify.
pub fn reconcile_lake_env(env: &Value) -> Result<er_entity_diff::Reconciled, String> {
    let catalog = env_str(env, "ER_LAKE_CATALOG").ok_or("org env missing ER_LAKE_CATALOG")?;
    let data_path = env_str(env, "ER_LAKE_DATA_PATH").ok_or("org env missing ER_LAKE_DATA_PATH")?;
    let session = er_cli::open_lake(catalog, data_path).map_err(|e| format!("open lake: {e:#}"))?;
    er_entity_diff::reconcile_lake(
        session.connection(),
        &er_entities::catalog::catalog(),
        &applied_by(),
    )
    .map_err(|e| match e {
        // The breaking message already lists every difference.
        e @ er_entity_diff::ReconcileError::Breaking(_) => e.to_string(),
        other => format!("lake schema: {}", crate::errfmt::chain(&other)),
    })
}

/// The `tenant_schema_states` row a reconciliation outcome maps to:
/// `(status, catalog_hash, detail)`.
#[must_use]
pub fn tenant_state(
    outcome: &Result<er_entity_diff::Reconciled, String>,
) -> (&'static str, String, String) {
    match outcome {
        Ok(r) => (r.status(), r.catalog_hash.clone(), r.detail()),
        Err(message) if message.starts_with("ERR_SCHEMA_BREAKING") => {
            ("breaking", lake_catalog_hash(), message.clone())
        }
        Err(message) => ("error", lake_catalog_hash(), message.clone()),
    }
}

/// Record (upsert) a tenant lake's schema position in the control plane.
///
/// # Errors
/// Propagates the upsert error.
pub async fn record_tenant_state(
    client: &tokio_postgres::Client,
    org_id: &str,
    status: &str,
    catalog_hash: &str,
    detail: Option<&str>,
) -> Result<(), PgError> {
    er_entities::tenant_schema_state::create::upsert(
        client,
        &er_ids::new_v7(),
        org_id,
        catalog_hash,
        status,
        detail,
    )
    .await?;
    Ok(())
}

/// The job kind the deploy's tenant sweep enqueues: reconcile the lake and stop.
/// Every other lake kind reconciles first too; this one exists so a tenant with
/// no scheduled work is migrated within the deploy rather than at its next run.
pub const LAKE_MIGRATE_KIND: &str = "lake_migrate";

/// Priority of the sweep's jobs: ahead of scheduled work, which is what makes
/// "the deploy reconciled every tenant" true within the hook's deadline rather
/// than after the next nightly run.
const LAKE_MIGRATE_PRIORITY: i64 = 100;

/// One line of the tenant report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantReport {
    pub org: String,
    pub job_id: String,
    /// `current` | `migrated` | `breaking` | `error` | `timeout`.
    pub outcome: String,
    pub detail: String,
}

impl TenantReport {
    /// Whether this line is a failure for the deploy (anything but current or
    /// migrated).
    #[must_use]
    pub fn failed(&self) -> bool {
        !matches!(self.outcome.as_str(), "current" | "migrated")
    }
}

/// Enqueue one `lake_migrate` job per active org, created by the sweep. Suspended
/// and provisioning orgs are skipped: a suspended lake migrates on resume (every
/// job reconciles first), a provisioning one is reconciled by provisioning.
///
/// # Errors
/// Propagates the org list or enqueue error.
pub async fn enqueue_tenant_migrations(
    client: &tokio_postgres::Client,
) -> Result<Vec<(String, crate::Job)>, PgError> {
    let mut jobs = Vec::new();
    for org in pg::list_orgs(client).await? {
        if org.state != "active" {
            continue;
        }
        let job = pg::enqueue(
            client,
            &org.id,
            LAKE_MIGRATE_KIND,
            LAKE_MIGRATE_PRIORITY,
            None,
            None,
            Some("er-server migrate-tenants"),
        )
        .await?;
        jobs.push((org.name, job));
    }
    Ok(jobs)
}

/// Wait for the sweep's jobs to reach a terminal state (the dispatcher runs
/// them; this only watches), then read each tenant's recorded position. A job
/// still running at the deadline reports `timeout`; it keeps running, and the
/// next event on that tenant (its own job start, the next sweep) finishes the
/// work.
///
/// # Errors
/// Propagates a job or state read error.
pub async fn wait_for_tenant_migrations(
    client: &tokio_postgres::Client,
    jobs: &[(String, crate::Job)],
    timeout: Duration,
) -> Result<Vec<TenantReport>, PgError> {
    let deadline = Instant::now() + timeout;
    let mut pending: Vec<(String, crate::Job)> = jobs.to_vec();
    let mut finished: Vec<(String, crate::Job)> = Vec::new();
    while !pending.is_empty() && Instant::now() < deadline {
        let mut still = Vec::new();
        for (org, job) in pending {
            match pg::get_job(client, &job.org_id, &job.id).await? {
                Some(j) if matches!(j.state.as_str(), "succeeded" | "failed" | "canceled") => {
                    finished.push((org, j));
                }
                Some(j) => still.push((org, j)),
                // Deleted underneath us: nothing more to wait for.
                None => finished.push((org, job)),
            }
        }
        pending = still;
        if !pending.is_empty() {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
    let positions: std::collections::BTreeMap<String, (String, Option<String>)> =
        er_entities::tenant_schema_state::read::list(client)
            .await?
            .into_iter()
            .map(|s| (s.org_id, (s.status, s.detail)))
            .collect();
    let mut report: Vec<TenantReport> = finished
        .into_iter()
        .map(|(org, job)| {
            let (outcome, detail) = match positions.get(&job.org_id) {
                Some((status, detail)) if job.state == "succeeded" => {
                    (status.clone(), detail.clone().unwrap_or_default())
                }
                Some((status, detail)) => (
                    status.clone(),
                    detail
                        .clone()
                        .or_else(|| job.error_detail.clone())
                        .unwrap_or_default(),
                ),
                None => (
                    "error".to_string(),
                    job.error_detail
                        .clone()
                        .unwrap_or_else(|| format!("job {}", job.state)),
                ),
            };
            TenantReport {
                org,
                job_id: job.id,
                outcome,
                detail,
            }
        })
        .chain(pending.into_iter().map(|(org, job)| TenantReport {
            org,
            job_id: job.id,
            outcome: "timeout".to_string(),
            detail: format!("job still {} at the deadline", job.state),
        }))
        .collect();
    report.sort_by(|a, b| a.org.cmp(&b.org));
    Ok(report)
}

/// The report as the hook's log prints it: one line per tenant.
#[must_use]
pub fn render_report(report: &[TenantReport]) -> String {
    use std::fmt::Write as _;
    let width = report.iter().map(|r| r.org.len()).max().unwrap_or(6).max(6);
    let mut out = format!("{:<width$}  {:<9}  detail\n", "tenant", "outcome");
    for r in report {
        let _ = writeln!(out, "{:<width$}  {:<9}  {}", r.org, r.outcome, r.detail);
    }
    out
}

fn env_str<'a>(env: &'a Value, key: &str) -> Option<&'a str> {
    env.get(key).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_state_classifies_the_outcome() {
        let ok = Ok(er_entity_diff::Reconciled {
            catalog_hash: "h".to_string(),
            applied: Vec::new(),
            ahead: Vec::new(),
            ledger_was_current: true,
        });
        let (status, hash, detail) = tenant_state(&ok);
        assert_eq!(
            (status, hash.as_str(), detail.as_str()),
            ("current", "h", "nothing pending")
        );
        let breaking: Result<er_entity_diff::Reconciled, String> =
            Err("ERR_SCHEMA_BREAKING: 1 difference(s)".to_string());
        assert_eq!(tenant_state(&breaking).0, "breaking");
        let err: Result<er_entity_diff::Reconciled, String> = Err("open lake: boom".to_string());
        let (status, hash, _) = tenant_state(&err);
        assert_eq!(status, "error");
        assert_eq!(hash, lake_catalog_hash());
    }

    #[test]
    fn the_report_marks_anything_but_current_or_migrated_as_failed() {
        let line = |outcome: &str| TenantReport {
            org: "acme".to_string(),
            job_id: "j".to_string(),
            outcome: outcome.to_string(),
            detail: String::new(),
        };
        assert!(!line("current").failed());
        assert!(!line("migrated").failed());
        assert!(line("breaking").failed());
        assert!(line("error").failed());
        assert!(line("timeout").failed());
        let text = render_report(&[line("migrated")]);
        assert!(text.starts_with("tenant  outcome    detail\n"), "{text:?}");
        assert!(text.contains("acme    migrated"), "{text:?}");
    }

    #[test]
    fn reconcile_refuses_an_env_without_a_lake() {
        let err = reconcile_lake_env(&serde_json::json!({})).unwrap_err();
        assert_eq!(err, "org env missing ER_LAKE_CATALOG");
    }
}
