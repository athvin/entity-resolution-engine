//! The Postgres-backed control-plane store (P3): the `orgs` + `jobs` schema and
//! the SKIP-LOCKED job queue — a Rust (tokio-postgres) port of
//! `server/src/erserver/db.py` + `queue.py`. The two correctness constraints are
//! enforced by Postgres, not code: at most one ACTIVE job per org (single-writer
//! per tenant) and at most one job per `(org, idempotency_key)`.
//!
//! This is the production queue backend; the in-memory store in `lib.rs` remains
//! for the HTTP unit tests. Wiring the HTTP handlers to this backend is the next
//! step. Gated test: `tests/pg_e2e.rs` behind `ER_TEST_PG_DSN`.

use tokio_postgres::{Client, Error as PgError, NoTls, Row};

use crate::Job;

pub const QUEUED: &str = "queued";
pub const DISPATCHING: &str = "dispatching";
/// States that hold an org's single-writer slot (jobs_one_active_per_org).
const ACTIVE_STATES: [&str; 3] = ["dispatching", "running", "canceling"];

/// The control-plane schema (orgs + jobs + the load-bearing indexes). The
/// `jobs`/`orgs` columns fold in the DDL that db.py adds via later ALTERs.
const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS orgs (\
  name text PRIMARY KEY,\
  config_path text NOT NULL,\
  env jsonb NOT NULL DEFAULT '{}'::jsonb,\
  drop_root text,\
  active_config_version integer,\
  state text NOT NULL DEFAULT 'active' CHECK (state IN ('provisioning','active','suspended','purging')),\
  created_at timestamptz NOT NULL DEFAULT now()\
);\
CREATE TABLE IF NOT EXISTS jobs (\
  job_id text PRIMARY KEY,\
  org text NOT NULL REFERENCES orgs(name),\
  kind text NOT NULL,\
  params jsonb NOT NULL DEFAULT '{}'::jsonb,\
  state text NOT NULL DEFAULT 'queued',\
  priority integer NOT NULL DEFAULT 0,\
  idempotency_key text,\
  run_id text,\
  attempt integer NOT NULL DEFAULT 0,\
  max_attempts integer NOT NULL DEFAULT 3,\
  not_before timestamptz,\
  created_by text,\
  schedule_id text,\
  resource_class text,\
  trace_context text,\
  created_at timestamptz NOT NULL DEFAULT now(),\
  updated_at timestamptz NOT NULL DEFAULT now(),\
  started_at timestamptz,\
  finished_at timestamptz\
);\
CREATE UNIQUE INDEX IF NOT EXISTS jobs_idempotency ON jobs (org, idempotency_key) WHERE idempotency_key IS NOT NULL;\
DROP INDEX IF EXISTS jobs_one_active_per_org;\
CREATE UNIQUE INDEX jobs_one_active_per_org ON jobs (org) WHERE state IN ('dispatching','running','canceling');\
CREATE INDEX IF NOT EXISTS jobs_claimable ON jobs (state, not_before, priority, job_id);";

const JOB_COLS: &str = "job_id, org, kind, state, priority";

fn job_from_row(row: &Row) -> Job {
    Job {
        job_id: row.get("job_id"),
        org: row.get("org"),
        kind: row.get("kind"),
        state: row.get("state"),
        priority: i64::from(row.get::<_, i32>("priority")),
    }
}

/// Connect to the control-plane database, spawning the connection driver.
///
/// # Errors
/// Propagates the connection error.
pub async fn connect(dsn: &str) -> Result<Client, PgError> {
    let (client, connection) = tokio_postgres::connect(dsn, NoTls).await?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok(client)
}

/// Apply the schema under the advisory-lock bootstrap (db.py `ensure_schema`):
/// the API and dispatcher both ensure the schema at boot, and the
/// drop-then-recreate index idiom deadlocks without serialization.
///
/// # Errors
/// Propagates any DDL / transaction error.
pub async fn ensure_schema(client: &mut Client) -> Result<(), PgError> {
    let tx = client.transaction().await?;
    tx.execute(
        "SELECT pg_advisory_xact_lock(hashtext('erserver.ensure_schema'), 0)",
        &[],
    )
    .await?;
    tx.batch_execute(SCHEMA).await?;
    tx.commit().await?;
    Ok(())
}

/// Register an org (idempotent).
///
/// # Errors
/// Propagates the insert error.
pub async fn ensure_org(client: &Client, name: &str, config_path: &str) -> Result<(), PgError> {
    client
        .execute(
            "INSERT INTO orgs (name, config_path, state) VALUES ($1, $2, 'active') \
             ON CONFLICT (name) DO NOTHING",
            &[&name, &config_path],
        )
        .await?;
    Ok(())
}

/// Queue one job, or return the job an identical idempotency key created
/// (queue.py `enqueue`). The org must exist (FK).
///
/// # Errors
/// Propagates the insert/select error (incl. the FK violation for an unknown org).
pub async fn enqueue(
    client: &Client,
    org: &str,
    kind: &str,
    priority: i32,
    idempotency_key: Option<&str>,
) -> Result<Job, PgError> {
    let job_id = ulid::Ulid::new().to_string();
    let inserted = client
        .query_opt(
            &format!(
                "INSERT INTO jobs (job_id, org, kind, priority, idempotency_key) \
                 VALUES ($1, $2, $3, $4, $5) \
                 ON CONFLICT (org, idempotency_key) WHERE idempotency_key IS NOT NULL \
                 DO NOTHING RETURNING {JOB_COLS}"
            ),
            &[&job_id, &org, &kind, &priority, &idempotency_key],
        )
        .await?;
    if let Some(row) = inserted {
        return Ok(job_from_row(&row));
    }
    // The key was already used: return the original submission's job.
    let row = client
        .query_one(
            &format!("SELECT {JOB_COLS} FROM jobs WHERE org = $1 AND idempotency_key = $2"),
            &[&org, &idempotency_key],
        )
        .await?;
    Ok(job_from_row(&row))
}

/// Claim the next runnable job (queue.py `claim`): the highest-priority `queued`
/// job whose org has no active job, via `FOR UPDATE SKIP LOCKED`. A lost
/// one-active-per-org race surfaces as a unique violation → `None`, never an
/// error.
///
/// # Errors
/// Propagates a non-unique-violation query error.
pub async fn claim(client: &Client) -> Result<Option<Job>, PgError> {
    let active: Vec<&str> = ACTIVE_STATES.to_vec();
    let res = client
        .query_opt(
            &format!(
                "UPDATE jobs SET state = $1, started_at = now(), updated_at = now() \
                 WHERE job_id = ( \
                   SELECT j.job_id FROM jobs j \
                   WHERE j.state = $2 AND (j.not_before IS NULL OR j.not_before <= now()) \
                     AND NOT EXISTS ( \
                       SELECT 1 FROM jobs a WHERE a.org = j.org AND a.state = ANY($3)) \
                   ORDER BY j.priority DESC, j.job_id FOR UPDATE SKIP LOCKED LIMIT 1) \
                 RETURNING {JOB_COLS}"
            ),
            &[&DISPATCHING, &QUEUED, &active],
        )
        .await;
    match res {
        Ok(Some(row)) => Ok(Some(job_from_row(&row))),
        Ok(None) => Ok(None),
        Err(e) if e.code() == Some(&tokio_postgres::error::SqlState::UNIQUE_VIOLATION) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Record a terminal/next state on a job (`running`, `succeeded`, `failed`,
/// `canceled`), stamping `finished_at` for terminal states.
///
/// # Errors
/// Propagates the update error.
pub async fn mark(client: &Client, job_id: &str, state: &str) -> Result<(), PgError> {
    client
        .execute(
            "UPDATE jobs SET state = $1, updated_at = now(), \
             finished_at = CASE WHEN $1 IN ('succeeded','failed','canceled') THEN now() \
                                ELSE finished_at END \
             WHERE job_id = $2",
            &[&state, &job_id],
        )
        .await?;
    Ok(())
}

/// An org's jobs, newest first.
///
/// # Errors
/// Propagates the query error.
pub async fn list_jobs(client: &Client, org: &str) -> Result<Vec<Job>, PgError> {
    let rows = client
        .query(
            &format!("SELECT {JOB_COLS} FROM jobs WHERE org = $1 ORDER BY job_id DESC"),
            &[&org],
        )
        .await?;
    Ok(rows.iter().map(job_from_row).collect())
}
