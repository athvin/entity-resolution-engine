//! The Postgres-backed control-plane store (P3): the schema (orgs, jobs,
//! api_keys, config_versions), the SKIP-LOCKED job queue, SHA-256 API-key
//! mint/lookup/revoke, and the config-version registry — a Rust (tokio-postgres)
//! port of `server/src/erserver/` `db.py`, `queue.py`, `auth.py`, `configsvc.py`.
//! The two queue correctness constraints are enforced by Postgres, not code: at
//! most one ACTIVE job per org (single-writer per tenant) and at most one job per
//! `(org, idempotency_key)`.
//!
//! This is the production queue backend; the HTTP job handlers route here when
//! `AppState` carries a DSN (connect-per-request). The in-memory store in
//! `lib.rs` remains for the network-free HTTP unit tests. Gated tests:
//! `tests/pg_e2e.rs` (the queue directly) and `tests/pg_http.rs` (the queue
//! through the HTTP surface), both behind `ER_TEST_PG_DSN`.

use tokio_postgres::{Client, Error as PgError, NoTls, Row};

use crate::Job;

pub const QUEUED: &str = "queued";
pub const DISPATCHING: &str = "dispatching";
/// States that hold an org's single-writer slot (jobs_one_active_per_org).
const ACTIVE_STATES: [&str; 3] = ["dispatching", "running", "canceling"];

/// The control-plane schema (orgs + jobs + api_keys + config_versions + the
/// load-bearing indexes). The `jobs`/`orgs` columns fold in the DDL that db.py
/// adds via later ALTERs.
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
CREATE INDEX IF NOT EXISTS jobs_claimable ON jobs (state, not_before, priority, job_id);\
CREATE TABLE IF NOT EXISTS api_keys (\
  key_id text PRIMARY KEY,\
  org text NOT NULL REFERENCES orgs(name),\
  role text NOT NULL CHECK (role IN ('admin','steward','viewer')),\
  key_hash text NOT NULL,\
  created_at timestamptz NOT NULL DEFAULT now(),\
  revoked_at timestamptz\
);\
CREATE TABLE IF NOT EXISTS config_versions (\
  org text NOT NULL REFERENCES orgs(name),\
  version integer NOT NULL,\
  yaml text NOT NULL,\
  config_hash text NOT NULL,\
  state text NOT NULL DEFAULT 'draft' CHECK (state IN ('draft','published')),\
  tier text,\
  created_by text NOT NULL,\
  created_at timestamptz NOT NULL DEFAULT now(),\
  published_at timestamptz,\
  PRIMARY KEY (org, version)\
);\
CREATE TABLE IF NOT EXISTS schedules (\
  schedule_id text PRIMARY KEY,\
  org text NOT NULL REFERENCES orgs(name),\
  kind text NOT NULL,\
  params jsonb NOT NULL DEFAULT '{}'::jsonb,\
  cron text NOT NULL,\
  enabled boolean NOT NULL DEFAULT true,\
  source text NOT NULL DEFAULT 'api',\
  last_enqueued_at timestamptz,\
  created_at timestamptz NOT NULL DEFAULT now()\
);\
CREATE UNIQUE INDEX IF NOT EXISTS schedules_one_correction_per_org ON schedules (org) WHERE source = 'config:correction_pass';\
CREATE UNIQUE INDEX IF NOT EXISTS schedules_one_maintenance_per_org ON schedules (org) WHERE source = 'system:lake_maintain';\
CREATE TABLE IF NOT EXISTS audit_log (\
  id bigserial PRIMARY KEY,\
  at timestamptz NOT NULL DEFAULT now(),\
  actor text NOT NULL,\
  org text,\
  action text NOT NULL,\
  detail jsonb NOT NULL DEFAULT '{}'::jsonb\
);";

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
/// (queue.py `enqueue`). The org must exist (FK). `params` is the job's input
/// map (e.g. `{"skip_ingest": true}` for a publish rebuild); `None` stores `{}`.
///
/// # Errors
/// Propagates the insert/select error (incl. the FK violation for an unknown org).
pub async fn enqueue(
    client: &Client,
    org: &str,
    kind: &str,
    priority: i32,
    idempotency_key: Option<&str>,
    params: Option<&serde_json::Value>,
) -> Result<Job, PgError> {
    let job_id = ulid::Ulid::new().to_string();
    let inserted = client
        .query_opt(
            &format!(
                "INSERT INTO jobs (job_id, org, kind, priority, idempotency_key, params) \
                 VALUES ($1, $2, $3, $4, $5, COALESCE($6, '{{}}'::jsonb)) \
                 ON CONFLICT (org, idempotency_key) WHERE idempotency_key IS NOT NULL \
                 DO NOTHING RETURNING {JOB_COLS}"
            ),
            &[&job_id, &org, &kind, &priority, &idempotency_key, &params],
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

/// Requeue a failed/canceled job (`queue.py` `resume_job`): back to `queued`
/// with `not_before = now()`, `finished_at` cleared, and — if it has a `run_id`
/// — a `resume_run_id` param so the next dispatch resumes that run. Returns
/// `true` iff a resumable row changed; `false` (→ 409) for any other state.
///
/// # Errors
/// Propagates the update error.
pub async fn resume_job(client: &Client, org: &str, job_id: &str) -> Result<bool, PgError> {
    let n = client
        .execute(
            "UPDATE jobs SET state = 'queued', not_before = now(), updated_at = now(), \
             finished_at = NULL, \
             params = CASE WHEN run_id IS NULL THEN params \
                           ELSE params || jsonb_build_object('resume_run_id', run_id) END \
             WHERE org = $1 AND job_id = $2 AND state IN ('failed', 'canceled')",
            &[&org, &job_id],
        )
        .await?;
    Ok(n == 1)
}

/// One job by id scoped to an org (`None` if absent / another org's).
///
/// # Errors
/// Propagates the query error.
pub async fn get_job(client: &Client, org: &str, job_id: &str) -> Result<Option<Job>, PgError> {
    let row = client
        .query_opt(
            &format!("SELECT {JOB_COLS} FROM jobs WHERE org = $1 AND job_id = $2"),
            &[&org, &job_id],
        )
        .await?;
    Ok(row.as_ref().map(job_from_row))
}

/// The cross-org queue view for the operator console (`queue.py`
/// `list_jobs_all`): optionally filtered by `org` and/or `state`, newest first.
///
/// # Errors
/// Propagates the query error.
pub async fn list_jobs_all(
    client: &Client,
    org: Option<&str>,
    state: Option<&str>,
    limit: i64,
) -> Result<Vec<Job>, PgError> {
    let limit = limit.clamp(1, 200);
    let mut clauses = vec!["TRUE".to_string()];
    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = Vec::new();
    // Push references to the `&str` values (tokio-postgres impls ToSql on &str).
    if let Some(o) = org.as_ref() {
        clauses.push(format!("org = ${}", params.len() + 1));
        params.push(o);
    }
    if let Some(s) = state.as_ref() {
        clauses.push(format!("state = ${}", params.len() + 1));
        params.push(s);
    }
    params.push(&limit);
    let sql = format!(
        "SELECT {JOB_COLS} FROM jobs WHERE {} ORDER BY job_id DESC LIMIT ${}",
        clauses.join(" AND "),
        params.len()
    );
    let rows = client.query(&sql, &params).await?;
    Ok(rows.iter().map(job_from_row).collect())
}

/// Merge DuckDB resource knobs into an org's `env` (`queue.py` `merge_org_env`):
/// `UPDATE orgs SET env = env || $2`.
///
/// # Errors
/// Propagates the update error.
pub async fn merge_org_env(
    client: &Client,
    org: &str,
    updates: &serde_json::Value,
) -> Result<(), PgError> {
    client
        .execute(
            "UPDATE orgs SET env = env || $2 WHERE name = $1",
            &[&org, &updates],
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

/// SHA-256 hex of `secret` — the exact at-rest key representation (`auth.py`
/// `_hash`): the UTF-8 bytes of the secret substring ONLY (no prefix, no salt),
/// lowercase hex. Never argon2 — matching the Python contract verbatim.
#[must_use]
pub fn hash_secret(secret: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(secret.as_bytes());
    hex::encode(digest)
}

/// Mint an API key for an org (`auth.py` `issue_key`): a lowercased-ULID
/// `key_id`, a random url-safe `secret`, and the plaintext token
/// `erk_{key_id}_{secret}` returned to the caller ONCE. Only the SHA-256 hash of
/// the secret is stored. `role` must already be one of admin/steward/viewer (the
/// table CHECK enforces it too). The org must exist (FK).
///
/// # Errors
/// Propagates the insert error (incl. the FK violation / role CHECK violation).
pub async fn issue_key(
    client: &Client,
    org: &str,
    role: &str,
) -> Result<(String, String), PgError> {
    let key_id = ulid::Ulid::new().to_string().to_lowercase();
    // 24 random bytes, hex-encoded: url-safe and free of '_', so the token
    // survives the Python `credential.split("_", 2)` decode unambiguously.
    let secret = {
        use rand::RngCore;
        let mut buf = [0u8; 24];
        rand::rng().fill_bytes(&mut buf);
        hex::encode(buf)
    };
    let key_hash = hash_secret(&secret);
    client
        .execute(
            "INSERT INTO api_keys (key_id, org, role, key_hash) VALUES ($1, $2, $3, $4)",
            &[&key_id, &org, &role, &key_hash],
        )
        .await?;
    let full_key = format!("erk_{key_id}_{secret}");
    Ok((key_id, full_key))
}

/// Revoke a key, scoped to its org and idempotent (`auth.py` `revoke_key`):
/// returns `true` only if this call flipped a live key to revoked. A second
/// revoke, or a `key_id` belonging to another org, returns `false`.
///
/// # Errors
/// Propagates the update error.
pub async fn revoke_key(client: &Client, key_id: &str, org: &str) -> Result<bool, PgError> {
    let n = client
        .execute(
            "UPDATE api_keys SET revoked_at = now() \
             WHERE key_id = $1 AND org = $2 AND revoked_at IS NULL",
            &[&key_id, &org],
        )
        .await?;
    Ok(n == 1)
}

/// Resolve a live (non-revoked) key by id, returning `(org, role, key_hash)`
/// (`auth.py` `authenticate` lookup). The caller SHA-256s the presented secret
/// and constant-time-compares it against `key_hash`.
///
/// # Errors
/// Propagates the query error.
pub async fn lookup_key(
    client: &Client,
    key_id: &str,
) -> Result<Option<(String, String, String)>, PgError> {
    let row = client
        .query_opt(
            "SELECT org, role, key_hash FROM api_keys WHERE key_id = $1 AND revoked_at IS NULL",
            &[&key_id],
        )
        .await?;
    Ok(row.map(|r| (r.get("org"), r.get("role"), r.get("key_hash"))))
}

/// A row of the operator org listing (`queue.py` `list_orgs`). `created_at` is
/// rendered to text in SQL so the port stays free of a timestamp-type feature.
pub struct OrgSummary {
    pub name: String,
    pub state: String,
    pub active_config_version: Option<i32>,
    pub created_at: String,
}

/// An org's detail view (`queue.py` `org_record`). `env` is the raw settings map
/// the handler projects `resources` out of.
pub struct OrgDetail {
    pub name: String,
    pub state: String,
    pub active_config_version: Option<i32>,
    pub drop_root: Option<String>,
    pub env: serde_json::Value,
}

/// Upsert a bring-your-own-config org (`queue.py` `ensure_org`, the create-org
/// BYO path): insert `active`, or on replay refresh `config_path`/`env`/
/// `drop_root` while leaving `state` intact. `env` binds straight to the jsonb
/// column (tokio-postgres `with-serde_json-1`).
///
/// # Errors
/// Propagates the insert error.
pub async fn upsert_org_byo(
    client: &Client,
    name: &str,
    config_path: &str,
    env: &serde_json::Value,
    drop_root: Option<&str>,
) -> Result<(), PgError> {
    client
        .execute(
            "INSERT INTO orgs (name, config_path, env, drop_root, state) \
             VALUES ($1, $2, $3, $4, 'active') \
             ON CONFLICT (name) DO UPDATE SET config_path = EXCLUDED.config_path, \
               env = EXCLUDED.env, drop_root = EXCLUDED.drop_root",
            &[&name, &config_path, &env, &drop_root],
        )
        .await?;
    Ok(())
}

/// Whether an org exists (for the org-or-404 precheck).
///
/// # Errors
/// Propagates the query error.
pub async fn org_exists(client: &Client, name: &str) -> Result<bool, PgError> {
    let row = client
        .query_opt("SELECT 1 FROM orgs WHERE name = $1", &[&name])
        .await?;
    Ok(row.is_some())
}

/// All orgs, name-ordered (`queue.py` `list_orgs`).
///
/// # Errors
/// Propagates the query error.
pub async fn list_orgs(client: &Client) -> Result<Vec<OrgSummary>, PgError> {
    let rows = client
        .query(
            "SELECT name, state, active_config_version, created_at::text AS created_at \
             FROM orgs ORDER BY name",
            &[],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| OrgSummary {
            name: r.get("name"),
            state: r.get("state"),
            active_config_version: r.get("active_config_version"),
            created_at: r.get("created_at"),
        })
        .collect())
}

/// One org's detail (`queue.py` `org_record`), `None` if absent. `env` is read
/// as text and reparsed to JSON.
///
/// # Errors
/// Propagates the query error.
pub async fn get_org(client: &Client, name: &str) -> Result<Option<OrgDetail>, PgError> {
    let row = client
        .query_opt(
            "SELECT name, state, active_config_version, drop_root, env::text AS env \
             FROM orgs WHERE name = $1",
            &[&name],
        )
        .await?;
    Ok(row.map(|r| {
        let env_text: String = r.get("env");
        OrgDetail {
            name: r.get("name"),
            state: r.get("state"),
            active_config_version: r.get("active_config_version"),
            drop_root: r.get("drop_root"),
            env: serde_json::from_str(&env_text).unwrap_or_else(|_| serde_json::json!({})),
        }
    }))
}

/// Transition an org's `state` only from `expected` (`queue.py` `set_org_state`):
/// returns `true` iff a row changed. A no-op (wrong current state) returns
/// `false`, which the handler maps to 409.
///
/// # Errors
/// Propagates the update error.
pub async fn set_org_state(
    client: &Client,
    name: &str,
    new_state: &str,
    expected: &str,
) -> Result<bool, PgError> {
    let n = client
        .execute(
            "UPDATE orgs SET state = $1 WHERE name = $2 AND state = $3",
            &[&new_state, &name, &expected],
        )
        .await?;
    Ok(n == 1)
}

/// The publish-time context for an org: `(config_path, state,
/// active_config_version)`, `None` if the org is absent.
///
/// # Errors
/// Propagates the query error.
pub async fn org_publish_context(
    client: &Client,
    org: &str,
) -> Result<Option<(String, String, Option<i32>)>, PgError> {
    let row = client
        .query_opt(
            "SELECT config_path, state, active_config_version FROM orgs WHERE name = $1",
            &[&org],
        )
        .await?;
    Ok(row.map(|r| {
        (
            r.get("config_path"),
            r.get("state"),
            r.get("active_config_version"),
        )
    }))
}

/// A config-version list row (`configsvc.py` list — no `yaml` body).
pub struct ConfigVersionRow {
    pub version: i32,
    pub config_hash: String,
    pub state: String,
    pub tier: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub published_at: Option<String>,
}

/// A config-version detail (adds the `yaml` body).
pub struct ConfigVersionDetail {
    pub version: i32,
    pub yaml: String,
    pub config_hash: String,
    pub state: String,
    pub tier: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub published_at: Option<String>,
}

/// Insert a new draft config version (`configsvc.py` `create_version`): the
/// version is `max(version)+1` for the org. Returns the assigned version. The
/// `(org, version)` PK rejects a concurrent duplicate.
///
/// # Errors
/// Propagates the query/insert error.
pub async fn create_config_version(
    client: &Client,
    org: &str,
    yaml: &str,
    config_hash: &str,
    created_by: &str,
) -> Result<i32, PgError> {
    let next: i32 = client
        .query_one(
            "SELECT coalesce(max(version), 0) + 1 AS v FROM config_versions WHERE org = $1",
            &[&org],
        )
        .await?
        .get("v");
    client
        .execute(
            "INSERT INTO config_versions (org, version, yaml, config_hash, created_by) \
             VALUES ($1, $2, $3, $4, $5)",
            &[&org, &next, &yaml, &config_hash, &created_by],
        )
        .await?;
    Ok(next)
}

/// An org's config versions, newest first (`configsvc.py` list; no `yaml`).
///
/// # Errors
/// Propagates the query error.
pub async fn list_config_versions(
    client: &Client,
    org: &str,
) -> Result<Vec<ConfigVersionRow>, PgError> {
    let rows = client
        .query(
            "SELECT version, config_hash, state, tier, created_by, \
                    created_at::text AS created_at, published_at::text AS published_at \
             FROM config_versions WHERE org = $1 ORDER BY version DESC",
            &[&org],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| ConfigVersionRow {
            version: r.get("version"),
            config_hash: r.get("config_hash"),
            state: r.get("state"),
            tier: r.get("tier"),
            created_by: r.get("created_by"),
            created_at: r.get("created_at"),
            published_at: r.get("published_at"),
        })
        .collect())
}

/// One config version (with `yaml`), `None` if absent.
///
/// # Errors
/// Propagates the query error.
pub async fn get_config_version(
    client: &Client,
    org: &str,
    version: i32,
) -> Result<Option<ConfigVersionDetail>, PgError> {
    let row = client
        .query_opt(
            "SELECT version, yaml, config_hash, state, tier, created_by, \
                    created_at::text AS created_at, published_at::text AS published_at \
             FROM config_versions WHERE org = $1 AND version = $2",
            &[&org, &version],
        )
        .await?;
    Ok(row.map(|r| ConfigVersionDetail {
        version: r.get("version"),
        yaml: r.get("yaml"),
        config_hash: r.get("config_hash"),
        state: r.get("state"),
        tier: r.get("tier"),
        created_by: r.get("created_by"),
        created_at: r.get("created_at"),
        published_at: r.get("published_at"),
    }))
}

/// Append one audit-ledger row (`auth.py` `audit`): one per authenticated
/// mutation, pairing with the lake's `entity_events`.
///
/// # Errors
/// Propagates the insert error.
pub async fn audit(
    client: &Client,
    actor: &str,
    org: Option<&str>,
    action: &str,
    detail: &serde_json::Value,
) -> Result<(), PgError> {
    client
        .execute(
            "INSERT INTO audit_log (actor, org, action, detail) VALUES ($1, $2, $3, $4)",
            &[&actor, &org, &action, &detail],
        )
        .await?;
    Ok(())
}

/// The audit ledger, newest first (`auth.py` `audit_rows`); `before_id` is the
/// keyset cursor. Optional `org` / `action` filters.
///
/// # Errors
/// Propagates the query error.
pub async fn audit_rows(
    client: &Client,
    org: Option<&str>,
    action: Option<&str>,
    before_id: Option<i64>,
    limit: i64,
) -> Result<Vec<serde_json::Value>, PgError> {
    let limit = limit.clamp(1, 500);
    let mut clauses = vec!["TRUE".to_string()];
    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = Vec::new();
    if let Some(o) = org.as_ref() {
        clauses.push(format!("org = ${}", params.len() + 1));
        params.push(o);
    }
    if let Some(a) = action.as_ref() {
        clauses.push(format!("action = ${}", params.len() + 1));
        params.push(a);
    }
    if let Some(b) = before_id.as_ref() {
        clauses.push(format!("id < ${}", params.len() + 1));
        params.push(b);
    }
    params.push(&limit);
    let sql = format!(
        "SELECT id, at::text AS at, actor, org, action, detail FROM audit_log \
         WHERE {} ORDER BY id DESC LIMIT ${}",
        clauses.join(" AND "),
        params.len()
    );
    let rows = client.query(&sql, &params).await?;
    Ok(rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.get::<_, i64>("id"),
                "at": r.get::<_, String>("at"),
                "actor": r.get::<_, String>("actor"),
                "org": r.get::<_, Option<String>>("org"),
                "action": r.get::<_, String>("action"),
                "detail": r.get::<_, serde_json::Value>("detail"),
            })
        })
        .collect())
}

/// A schedule row for the API listing (`schedules.py`).
pub struct ScheduleRow {
    pub schedule_id: String,
    pub kind: String,
    pub cron: String,
    pub enabled: bool,
    pub source: String,
    pub last_enqueued_at: Option<String>,
}

/// Create an API-sourced schedule (`schedules.py` `create`); returns its id.
///
/// # Errors
/// Propagates the insert error.
pub async fn create_schedule(
    client: &Client,
    org: &str,
    kind: &str,
    cron: &str,
    params: &serde_json::Value,
) -> Result<String, PgError> {
    let schedule_id = ulid::Ulid::new().to_string();
    client
        .execute(
            "INSERT INTO schedules (schedule_id, org, kind, params, cron, source) \
             VALUES ($1, $2, $3, $4, $5, 'api')",
            &[&schedule_id, &org, &kind, &params, &cron],
        )
        .await?;
    Ok(schedule_id)
}

/// An org's schedules, oldest first (`schedules.py` `list_schedules`).
///
/// # Errors
/// Propagates the query error.
pub async fn list_schedules(client: &Client, org: &str) -> Result<Vec<ScheduleRow>, PgError> {
    let rows = client
        .query(
            "SELECT schedule_id, kind, cron, enabled, source, \
                    last_enqueued_at::text AS last_enqueued_at \
             FROM schedules WHERE org = $1 ORDER BY created_at",
            &[&org],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| ScheduleRow {
            schedule_id: r.get("schedule_id"),
            kind: r.get("kind"),
            cron: r.get("cron"),
            enabled: r.get("enabled"),
            source: r.get("source"),
            last_enqueued_at: r.get("last_enqueued_at"),
        })
        .collect())
}

/// Enable/disable an API-sourced schedule (`schedules.py` `set_enabled`):
/// config/system-owned rows (`source != 'api'`) refuse. `true` iff a row
/// changed.
///
/// # Errors
/// Propagates the update error.
pub async fn set_schedule_enabled(
    client: &Client,
    org: &str,
    schedule_id: &str,
    enabled: bool,
) -> Result<bool, PgError> {
    let n = client
        .execute(
            "UPDATE schedules SET enabled = $1 \
             WHERE schedule_id = $2 AND org = $3 AND source = 'api'",
            &[&enabled, &schedule_id, &org],
        )
        .await?;
    Ok(n == 1)
}

/// Delete a schedule (`schedules.py` `delete`). `true` iff a row was removed.
///
/// # Errors
/// Propagates the delete error.
pub async fn delete_schedule(
    client: &Client,
    org: &str,
    schedule_id: &str,
) -> Result<bool, PgError> {
    let n = client
        .execute(
            "DELETE FROM schedules WHERE org = $1 AND schedule_id = $2",
            &[&org, &schedule_id],
        )
        .await?;
    Ok(n == 1)
}

/// Mark a version published and point the org at it (`configsvc.py` `publish`
/// DB writes): `config_versions` state/tier/`published_at`, then
/// `orgs.active_config_version`.
///
/// # Errors
/// Propagates either update error.
pub async fn publish_config_version(
    client: &Client,
    org: &str,
    version: i32,
    tier: Option<&str>,
) -> Result<(), PgError> {
    client
        .execute(
            "UPDATE config_versions SET state = 'published', tier = $3, published_at = now() \
             WHERE org = $1 AND version = $2",
            &[&org, &version, &tier],
        )
        .await?;
    client
        .execute(
            "UPDATE orgs SET active_config_version = $2 WHERE name = $1",
            &[&org, &version],
        )
        .await?;
    Ok(())
}
