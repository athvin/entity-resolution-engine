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

use tokio_postgres::{Client, Error as PgError};

use crate::Job;

/// Render a timestamp the way the entity wire contract declares (RFC 3339 /
/// OpenAPI `date-time`), for the projection structs that carry text timestamps.
#[must_use]
pub fn ts(t: &chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

pub const QUEUED: &str = "queued";
pub const DISPATCHING: &str = "dispatching";

// The control-plane schema is the generated baseline: every `public.*` table's
// DDL in FK-dependency order, emitted from the entity catalog by er-entity-gen
// (`er_entities::schema::POSTGRES_BASELINE`). No schema is hand-written here.

// The job queue's SQL and row mapping are generated from entities/job/ — the five
// functions below are thin wrappers over `er_entities::job::*`.

/// Connect to the control-plane database, spawning the connection driver.
///
/// # Errors
/// Propagates the connection error.
pub async fn connect(dsn: &str) -> Result<Client, PgError> {
    let (client, connection) = tokio_postgres::connect(dsn, er_pg_tls::make_tls()).await?;
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
    // The baseline replays CREATE OR REPLACE / IF NOT EXISTS DDL every boot;
    // suppress the DDL ledger for the replay so boots don't spam it (real
    // migrations and operator DDL never set this).
    tx.batch_execute("SET LOCAL er.ddl_log_skip = 'on'").await?;
    for stmt in er_entities::schema::POSTGRES_BASELINE {
        tx.batch_execute(stmt).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Install the DDL-ledger event triggers (`er_ddl_audit_end` / `er_ddl_audit_drop`)
/// best-effort: `CREATE EVENT TRIGGER` needs superuser/rds_superuser, which CI
/// and dev have but the app role deliberately does not. Returns `Ok(false)` on
/// insufficient privilege — the caller logs and boots on; production installs
/// through the deploy migrate Job running with the master DSN.
///
/// # Errors
/// Propagates any non-privilege DDL / transaction error.
pub async fn ensure_event_triggers(client: &mut Client) -> Result<bool, PgError> {
    let tx = client.transaction().await?;
    tx.execute(
        "SELECT pg_advisory_xact_lock(hashtext('erserver.ensure_schema'), 0)",
        &[],
    )
    .await?;
    if let Err(e) = tx
        .batch_execute(er_entities::schema::POSTGRES_EVENT_TRIGGERS)
        .await
    {
        if e.code() == Some(&tokio_postgres::error::SqlState::INSUFFICIENT_PRIVILEGE) {
            return Ok(false);
        }
        return Err(e);
    }
    tx.commit().await?;
    Ok(true)
}

/// Declare the acting principal for this connection (`er.actor`): the history
/// triggers and the DDL ledger record it; absent, they fall back to the
/// session user. Session-scoped — safe because er-server connects per request
/// and the dispatcher owns its one connection; if pooling ever arrives, switch
/// to transaction scope (`set_config(.., true)`).
///
/// # Errors
/// Propagates the query error.
pub async fn set_actor(client: &Client, actor: &str) -> Result<(), PgError> {
    client
        .execute("SELECT set_config('er.actor', $1, false)", &[&actor])
        .await?;
    Ok(())
}

/// Connect and declare the actor in one step (the mutation-handler idiom).
///
/// # Errors
/// Propagates the connect / query error.
pub async fn connect_pg_as(dsn: &str, actor: &str) -> Result<Client, PgError> {
    let client = connect(dsn).await?;
    set_actor(&client, actor).await?;
    Ok(client)
}

/// Register an org (idempotent).
///
/// # Errors
/// Propagates the insert error.
pub async fn ensure_org(client: &Client, name: &str, config_path: &str) -> Result<(), PgError> {
    er_entities::org::create::ensure(client, &er_ids::new_v7(), name, config_path).await?;
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
    org_id: &str,
    kind: &str,
    priority: i64,
    idempotency_key: Option<&str>,
    params: Option<&serde_json::Value>,
) -> Result<Job, PgError> {
    let job_id = er_ids::new_v7();
    // The statement is an idempotent upsert: a repeat idempotency key returns the
    // original row via a no-op ON CONFLICT DO UPDATE, so exactly one row comes back.
    let jobs = er_entities::job::create::enqueue(
        client,
        &job_id,
        org_id,
        kind,
        priority,
        idempotency_key,
        params,
    )
    .await?;
    Ok(jobs
        .into_iter()
        .next()
        .expect("enqueue RETURNING yields exactly one row"))
}

/// Claim the next runnable job (queue.py `claim`): the highest-priority `queued`
/// job whose org has no active job, via `FOR UPDATE SKIP LOCKED`. A lost
/// one-active-per-org race surfaces as a unique violation → `None`, never an
/// error.
///
/// # Errors
/// Propagates a non-unique-violation query error.
pub async fn claim(client: &Client) -> Result<Option<Job>, PgError> {
    match er_entities::job::update::claim(client).await {
        Ok(jobs) => Ok(jobs.into_iter().next()),
        // A lost one-active-per-org race surfaces as a unique violation -> None.
        Err(e) if e.code() == Some(&tokio_postgres::error::SqlState::UNIQUE_VIOLATION) => Ok(None),
        Err(e) => Err(e),
    }
}

/// How a job failure should be treated. The classes are the ones the UI already
/// renders — `job-detail.tsx` shows an error panel keyed on `error_class`, the
/// dashboard counts `precondition` failures, and the runs table has a column for
/// it — none of which could ever display anything, because the column did not
/// exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// An input the job needs is not there yet: no model, no deliveries, no
    /// prior run. Expected during onboarding, which is why the dashboard
    /// reframes it rather than alarming about it. Retrying cannot help — the
    /// input appears when a human provides it.
    Precondition,
    /// A lake open, an object-store read, a connection: the kind of failure that
    /// is usually gone a few seconds later. The ONLY class that retries, which
    /// is what makes the UI's "transient failures retry automatically" true.
    TransientIo,
    /// The tenant's config does not load or does not validate. Retrying would
    /// just fail identically.
    Config,
    /// Anything else — a bug until proven otherwise.
    Internal,
}

impl FailureClass {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            FailureClass::Precondition => "precondition",
            FailureClass::TransientIo => "transient_io",
            FailureClass::Config => "config",
            FailureClass::Internal => "internal",
        }
    }

    /// Whether a job in this class should be retried.
    #[must_use]
    pub fn retryable(self) -> bool {
        self == FailureClass::TransientIo
    }

    /// Classify a dispatcher failure string.
    ///
    /// Matched on the messages this codebase actually produces rather than on a
    /// generic taxonomy: "org env missing …" and "no prior run to correct" are
    /// preconditions by construction, `open lake` / `read model` are IO, and
    /// `config:` is the config loader's own prefix. Anything unrecognised is
    /// deliberately `Internal` and NOT retried — silently retrying an unknown
    /// failure three times turns one confusing log line into three.
    #[must_use]
    pub fn classify(message: &str) -> Self {
        let m = message.to_ascii_lowercase();
        if m.contains("org env missing")
            || m.contains("no prior run")
            || m.contains("no source deliveries")
            || m.contains("no trained model")
            || m.contains("no model yet")
            || m.contains("has no drop_root")
        {
            FailureClass::Precondition
        } else if m.starts_with("config:") || m.contains("read config") {
            FailureClass::Config
        } else if m.contains("open lake")
            || m.contains("read model")
            || m.contains("connection")
            || m.contains("timed out")
            || m.contains("timeout")
            || m.contains("temporarily")
            || m.contains("i/o error")
        {
            FailureClass::TransientIo
        } else {
            FailureClass::Internal
        }
    }
}

/// Record a job failure and let Postgres decide whether it retries.
///
/// Returns the job in its resulting state, so the caller can tell a retry from
/// a terminal failure — the dispatcher only emits `job.failed` for the latter,
/// because a webhook per attempt would turn one incident into three.
///
/// # Errors
/// Propagates the update error.
pub async fn fail_or_retry(
    client: &Client,
    job_id: &str,
    class: FailureClass,
    detail: &str,
) -> Result<Job, PgError> {
    // Keep the stored detail bounded: it is rendered in a panel, and a runaway
    // engine error can be enormous.
    let truncated: String = detail.chars().take(2000).collect();
    let jobs = er_entities::job::update::fail_or_retry(
        client,
        class.retryable(),
        Some(class.as_str()),
        Some(truncated.as_str()),
        job_id,
    )
    .await?;
    Ok(jobs
        .into_iter()
        .next()
        .expect("fail_or_retry RETURNING yields exactly one row"))
}

/// Record a terminal/next state on a job (`running`, `succeeded`, `failed`,
/// `canceled`), stamping `finished_at` for terminal states.
///
/// # Errors
/// Propagates the update error.
pub async fn mark(client: &Client, job_id: &str, state: &str) -> Result<(), PgError> {
    er_entities::job::update::mark(client, state, job_id).await?;
    Ok(())
}

/// Requeue a failed/canceled job (`queue.py` `resume_job`): back to `queued`
/// with `not_before = now()`, `finished_at` cleared, and — if it has a `run_id`
/// — a `resume_run_id` param so the next dispatch resumes that run. Returns
/// `true` iff a resumable row changed; `false` (→ 409) for any other state.
///
/// # Errors
/// Propagates the update error.
pub async fn resume_job(client: &Client, org_id: &str, job_id: &str) -> Result<bool, PgError> {
    let n = er_entities::job::update::resume(client, org_id, job_id).await?;
    Ok(n == 1)
}

/// One job by id scoped to an org (`None` if absent / another org's).
///
/// # Errors
/// Propagates the query error.
pub async fn get_job(client: &Client, org_id: &str, job_id: &str) -> Result<Option<Job>, PgError> {
    Ok(er_entities::job::read::by_id(client, org_id, job_id)
        .await?
        .into_iter()
        .next())
}

/// The cross-org queue view for the operator console (`queue.py`
/// `list_jobs_all`): optionally filtered by `org` and/or `state`, newest first.
///
/// # Errors
/// Propagates the query error.
pub async fn list_jobs_all(
    client: &Client,
    org_id: Option<&str>,
    state: Option<&str>,
    limit: i64,
) -> Result<Vec<Job>, PgError> {
    er_entities::job::read::list(client, org_id, state, limit.clamp(1, 200)).await
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
    er_entities::org::update::merge_env(client, updates, org).await?;
    Ok(())
}

/// An org's jobs, newest first.
///
/// # Errors
/// Propagates the query error.
pub async fn list_jobs(client: &Client, org_id: &str) -> Result<Vec<Job>, PgError> {
    // Newest first, all of the org's jobs (no page cap on the per-org view).
    er_entities::job::read::list(client, Some(org_id), None, i64::MAX).await
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
    org_id: &str,
    role: &str,
) -> Result<(String, String), PgError> {
    let key_id = er_ids::new_v7();
    // 24 random bytes, hex-encoded: url-safe and free of '_', so the token
    // survives the Python `credential.split("_", 2)` decode unambiguously.
    let secret = {
        use rand::Rng;
        let mut buf = [0u8; 24];
        rand::rng().fill_bytes(&mut buf);
        hex::encode(buf)
    };
    let key_hash = hash_secret(&secret);
    er_entities::api_key::create::issue(client, &key_id, org_id, role, &key_hash).await?;
    let full_key = format!("erk_{key_id}_{secret}");
    Ok((key_id, full_key))
}

/// Revoke a key, scoped to its org and idempotent (`auth.py` `revoke_key`):
/// returns `true` only if this call flipped a live key to revoked. A second
/// revoke, or a `key_id` belonging to another org, returns `false`.
///
/// # Errors
/// Propagates the update error.
pub async fn revoke_key(client: &Client, key_id: &str, org_id: &str) -> Result<bool, PgError> {
    let n = er_entities::api_key::update::revoke(client, key_id, org_id).await?;
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
    Ok(er_entities::api_key::read::live_by_id(client, key_id)
        .await?
        .into_iter()
        .next()
        .map(|k| (k.org_id, k.role, k.key_hash)))
}

/// A row of the operator org listing (`queue.py` `list_orgs`). `created_at` is
/// rendered to text in SQL so the port stays free of a timestamp-type feature.
pub struct OrgSummary {
    pub id: String,
    pub name: String,
    pub state: String,
    pub active_config_version: Option<i64>,
    pub created_at: String,
}

/// An org's detail view (`queue.py` `org_record`). `env` is the raw settings map
/// the handler projects `resources` out of.
pub struct OrgDetail {
    pub id: String,
    pub name: String,
    pub state: String,
    pub active_config_version: Option<i64>,
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
    er_entities::org::create::upsert_byo(
        client,
        &er_ids::new_v7(),
        name,
        config_path,
        env,
        drop_root,
    )
    .await?;
    Ok(())
}

/// Whether an org exists (for the org-or-404 precheck).
///
/// # Errors
/// Propagates the query error.
pub async fn org_exists(client: &Client, name: &str) -> Result<bool, PgError> {
    Ok(!er_entities::org::read::by_name(client, name)
        .await?
        .is_empty())
}

/// All orgs, name-ordered (`queue.py` `list_orgs`).
///
/// # Errors
/// Propagates the query error.
pub async fn list_orgs(client: &Client) -> Result<Vec<OrgSummary>, PgError> {
    Ok(er_entities::org::read::list(client)
        .await?
        .into_iter()
        .map(|o| OrgSummary {
            id: o.id,
            name: o.name,
            state: o.state,
            active_config_version: o.active_config_version,
            created_at: ts(&o.created_at),
        })
        .collect())
}

/// One org's detail (`queue.py` `org_record`), `None` if absent. `env` is read
/// as text and reparsed to JSON.
///
/// # Errors
/// Propagates the query error.
pub async fn get_org(client: &Client, name: &str) -> Result<Option<OrgDetail>, PgError> {
    Ok(er_entities::org::read::by_name(client, name)
        .await?
        .into_iter()
        .next()
        .map(|o| OrgDetail {
            id: o.id,
            name: o.name,
            state: o.state,
            active_config_version: o.active_config_version,
            drop_root: o.drop_root,
            env: o.env,
        }))
}

/// An org's id by its unique name (the routing handle) — the one-expression
/// resolver tests and operator tools use before calling the org_id-keyed
/// wrappers.
///
/// # Errors
/// Propagates the query error.
pub async fn org_id_by_name(client: &Client, name: &str) -> Result<Option<String>, PgError> {
    Ok(er_entities::org::read::by_name(client, name)
        .await?
        .into_iter()
        .next()
        .map(|o| o.id))
}

/// One org's detail by its id (the dispatcher's and key-auth's lookup once
/// child rows carry only `org_id`).
///
/// # Errors
/// Propagates the query error.
pub async fn get_org_by_id(client: &Client, org_id: &str) -> Result<Option<OrgDetail>, PgError> {
    Ok(er_entities::org::read::by_id(client, org_id)
        .await?
        .into_iter()
        .next()
        .map(|o| OrgDetail {
            id: o.id,
            name: o.name,
            state: o.state,
            active_config_version: o.active_config_version,
            drop_root: o.drop_root,
            env: o.env,
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
    let n = er_entities::org::update::set_state(client, new_state, name, expected).await?;
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
) -> Result<Option<(String, String, Option<i64>)>, PgError> {
    Ok(er_entities::org::read::by_name(client, org)
        .await?
        .into_iter()
        .next()
        .map(|o| (o.config_path, o.state, o.active_config_version)))
}

/// A config-version list row (`configsvc.py` list — no `yaml` body).
pub struct ConfigVersionRow {
    pub version: i64,
    pub config_hash: String,
    pub state: String,
    pub tier: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub published_at: Option<String>,
}

/// A config-version detail (adds the `yaml` body).
pub struct ConfigVersionDetail {
    pub version: i64,
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
    org_id: &str,
    yaml: &str,
    config_hash: &str,
    created_by: &str,
) -> Result<i64, PgError> {
    // The statement allocates max(version)+1 atomically inside the INSERT; the
    // (org, version) PK still rejects a concurrent duplicate.
    let rows = er_entities::config_version::create::draft(
        client,
        &er_ids::new_v7(),
        org_id,
        yaml,
        config_hash,
        created_by,
    )
    .await?;
    Ok(rows
        .into_iter()
        .next()
        .expect("draft RETURNING yields exactly one row")
        .version)
}

/// An org's config versions, newest first (`configsvc.py` list; no `yaml`).
///
/// # Errors
/// Propagates the query error.
pub async fn list_config_versions(
    client: &Client,
    org_id: &str,
) -> Result<Vec<ConfigVersionRow>, PgError> {
    Ok(er_entities::config_version::read::list(client, org_id)
        .await?
        .into_iter()
        .map(|v| ConfigVersionRow {
            version: v.version,
            config_hash: v.config_hash,
            state: v.state,
            tier: v.tier,
            created_by: v.created_by,
            created_at: ts(&v.created_at),
            published_at: v.published_at.as_ref().map(ts),
        })
        .collect())
}

/// One config version (with `yaml`), `None` if absent.
///
/// # Errors
/// Propagates the query error.
pub async fn get_config_version(
    client: &Client,
    org_id: &str,
    version: i64,
) -> Result<Option<ConfigVersionDetail>, PgError> {
    Ok(
        er_entities::config_version::read::by_version(client, org_id, version)
            .await?
            .into_iter()
            .next()
            .map(|v| ConfigVersionDetail {
                version: v.version,
                yaml: v.yaml,
                config_hash: v.config_hash,
                state: v.state,
                tier: v.tier,
                created_by: v.created_by,
                created_at: ts(&v.created_at),
                published_at: v.published_at.as_ref().map(ts),
            }),
    )
}

/// Stage a steward action for the dispatcher to drain when the org's writer
/// lock goes quiet (`steward.py` `stage_action`); returns its `action_id`.
///
/// # Errors
/// Propagates the insert error.
pub async fn stage_action(
    client: &Client,
    org_id: &str,
    action: &serde_json::Value,
    created_by: &str,
) -> Result<String, PgError> {
    let action_id = er_ids::new_v7();
    er_entities::staged_steward_action::create::stage(
        client, &action_id, org_id, action, created_by,
    )
    .await?;
    Ok(action_id)
}

/// An org's pending steward actions, oldest first (`steward.py` `drain_org`'s
/// read): `(action_id, action)` for the dispatcher to apply to the lake.
///
/// # Errors
/// Propagates the query error.
pub async fn pending_actions(
    client: &Client,
    org_id: &str,
) -> Result<Vec<(String, serde_json::Value)>, PgError> {
    Ok(
        er_entities::staged_steward_action::read::pending(client, org_id)
            .await?
            .into_iter()
            .map(|a| (a.id, a.action))
            .collect(),
    )
}

/// Mark a staged action applied (`steward.py` `drain_org`).
///
/// # Errors
/// Propagates the update error.
pub async fn mark_action_applied(client: &Client, action_id: &str) -> Result<(), PgError> {
    er_entities::staged_steward_action::update::mark_applied(client, action_id).await?;
    Ok(())
}

/// How many steward actions are pending apply for an org (`steward.py`
/// `pending_count`).
///
/// # Errors
/// Propagates the query error.
pub async fn pending_count(client: &Client, org_id: &str) -> Result<i64, PgError> {
    // The pending set is small by construction (drained each dispatcher pass),
    // so counting the drain read keeps this on the one declared statement.
    let pending = er_entities::staged_steward_action::read::pending(client, org_id).await?;
    Ok(pending.len() as i64)
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
    er_entities::audit_log::create::append(client, actor, org, action, detail).await?;
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
    let rows =
        er_entities::audit_log::read::page(client, org, action, before_id, limit.clamp(1, 500))
            .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "at": ts(&r.at),
                "actor": r.actor,
                "org": r.org,
                "action": r.action,
                "detail": r.detail,
            })
        })
        .collect())
}

/// A schedule row for the API listing (`schedules.py`).
pub struct ScheduleRow {
    pub id: String,
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
    org_id: &str,
    kind: &str,
    cron: &str,
    params: &serde_json::Value,
) -> Result<String, PgError> {
    let schedule_id = er_ids::new_v7();
    er_entities::schedule::create::create(client, &schedule_id, org_id, kind, params, cron).await?;
    Ok(schedule_id)
}

/// An org's schedules, oldest first (`schedules.py` `list_schedules`).
///
/// # Errors
/// Propagates the query error.
pub async fn list_schedules(client: &Client, org_id: &str) -> Result<Vec<ScheduleRow>, PgError> {
    Ok(er_entities::schedule::read::list(client, org_id)
        .await?
        .into_iter()
        .map(|s| ScheduleRow {
            id: s.id,
            kind: s.kind,
            cron: s.cron,
            enabled: s.enabled,
            source: s.source,
            last_enqueued_at: s.last_enqueued_at.as_ref().map(ts),
        })
        .collect())
}

/// Queue a job on behalf of a schedule: the same idempotent upsert as
/// [`enqueue`], but it stamps `schedule_id` and a `scheduler` principal so the
/// run is attributable to the rule that caused it.
///
/// # Errors
/// Propagates the insert/select error.
/// Returns the job and whether THIS call inserted it. The upsert returns the
/// pre-existing row on a repeat key, and that row is indistinguishable from a
/// fresh one by its contents — so the freshly minted id is the signal: if it
/// came back, this call is the one that created the row. The scheduler needs
/// that distinction to avoid counting (and re-stamping) a replayed firing.
pub async fn enqueue_scheduled(
    client: &Client,
    org_id: &str,
    kind: &str,
    idempotency_key: &str,
    params: Option<&serde_json::Value>,
    schedule_id: &str,
) -> Result<(Job, bool), PgError> {
    let job_id = er_ids::new_v7();
    let jobs = er_entities::job::create::enqueue_scheduled(
        client,
        &job_id,
        org_id,
        kind,
        0,
        Some(idempotency_key),
        params,
        schedule_id,
    )
    .await?;
    let job = jobs
        .into_iter()
        .next()
        .expect("enqueue_scheduled RETURNING yields exactly one row");
    let inserted = job.id == job_id;
    Ok((job, inserted))
}

/// The model-registry name a tenant's starter model is seeded under. One name
/// per tenant is enough until a tenant wants a second model beside it; versions
/// accumulate under this name.
pub const DEFAULT_MODEL_NAME: &str = "default";

/// SHA-256 hex of a model document — the version's identity, taken over the
/// exact bytes the engine will load.
#[must_use]
pub fn model_hash(model_json: &str) -> String {
    hash_secret(model_json)
}

/// Mint the next version of a named matching model.
///
/// # Errors
/// Propagates the insert error (incl. the natural-key unique violation two
/// concurrent creates would race into).
#[allow(clippy::too_many_arguments)]
pub async fn create_model_version(
    client: &Client,
    org_id: &str,
    name: &str,
    entity_type: &str,
    model_json: &str,
    model_version_label: &str,
    tf_snapshot_id: Option<&str>,
    provenance: &serde_json::Value,
    created_by: &str,
) -> Result<er_entities::matching_model::MatchingModel, PgError> {
    let rows = er_entities::matching_model::create::create(
        client,
        &er_ids::new_v7(),
        org_id,
        name,
        entity_type,
        model_json,
        &model_hash(model_json),
        model_version_label,
        tf_snapshot_id,
        provenance,
        created_by,
    )
    .await?;
    Ok(rows
        .into_iter()
        .next()
        .expect("create RETURNING yields exactly one row"))
}

/// Every model version a tenant has.
///
/// # Errors
/// Propagates the query error.
pub async fn list_model_versions(
    client: &Client,
    org_id: &str,
) -> Result<Vec<er_entities::matching_model::MatchingModel>, PgError> {
    er_entities::matching_model::read::list(client, org_id).await
}

/// The tenant's active model version, if it has one.
///
/// # Errors
/// Propagates the query error.
pub async fn active_model_version(
    client: &Client,
    org_id: &str,
) -> Result<Option<er_entities::matching_model::MatchingModel>, PgError> {
    Ok(er_entities::matching_model::read::active(client, org_id)
        .await?
        .into_iter()
        .next())
}

/// One model version by its natural key.
///
/// # Errors
/// Propagates the query error.
pub async fn model_version(
    client: &Client,
    org_id: &str,
    name: &str,
    version: i64,
) -> Result<Option<er_entities::matching_model::MatchingModel>, PgError> {
    Ok(
        er_entities::matching_model::read::by_version(client, org_id, name, version)
            .await?
            .into_iter()
            .next(),
    )
}

/// Activate one version, archiving whichever was active, atomically.
///
/// # Errors
/// Propagates the update error.
pub async fn activate_model_version(
    client: &Client,
    org_id: &str,
    model_id: &str,
) -> Result<(), PgError> {
    er_entities::matching_model::update::activate(client, model_id, org_id).await?;
    Ok(())
}

/// The `source` marking the schedule that a config's `correction_pass.cadence`
/// owns.
pub const CORRECTION_PASS_SOURCE: &str = "config:correction_pass";

/// Reconcile the config-owned correction-pass schedule with a newly published
/// `correction_pass.cadence`: refresh the cron if the row exists, create it if
/// it does not.
///
/// Before this, the cadence was stored in every tenant config and read by
/// nothing — the periodic correction pass it describes never ran. Publishing is
/// the right trigger because the cadence is part of the config document.
///
/// # Errors
/// Propagates the insert/update error.
pub async fn ensure_correction_schedule(
    client: &Client,
    org_id: &str,
    cadence: &str,
) -> Result<(), PgError> {
    er_entities::schedule::update::set_config_cron(client, cadence, org_id, CORRECTION_PASS_SOURCE)
        .await?;
    er_entities::schedule::create::ensure_config_owned(
        client,
        &er_ids::new_v7(),
        org_id,
        "correct",
        cadence,
        CORRECTION_PASS_SOURCE,
    )
    .await?;
    Ok(())
}

/// Every enabled schedule belonging to an active org — the dispatcher's
/// scheduler-tick candidate set, across all tenants.
///
/// # Errors
/// Propagates the query error.
pub async fn due_schedules(
    client: &Client,
) -> Result<Vec<er_entities::schedule::Schedule>, PgError> {
    er_entities::schedule::read::due(client).await
}

/// Stamp `last_enqueued_at` after a tick enqueued this schedule's job.
///
/// # Errors
/// Propagates the update error.
pub async fn mark_schedule_enqueued(client: &Client, schedule_id: &str) -> Result<(), PgError> {
    er_entities::schedule::update::mark_enqueued(client, schedule_id).await?;
    Ok(())
}

/// Enable/disable an API-sourced schedule (`schedules.py` `set_enabled`):
/// config/system-owned rows (`source != 'api'`) refuse. `true` iff a row
/// changed.
///
/// # Errors
/// Propagates the update error.
pub async fn set_schedule_enabled(
    client: &Client,
    org_id: &str,
    schedule_id: &str,
    enabled: bool,
) -> Result<bool, PgError> {
    let n =
        er_entities::schedule::update::set_enabled(client, enabled, schedule_id, org_id).await?;
    Ok(n == 1)
}

/// Delete a schedule (`schedules.py` `delete`). `true` iff a row was removed.
///
/// # Errors
/// Propagates the delete error.
pub async fn delete_schedule(
    client: &Client,
    org_id: &str,
    schedule_id: &str,
) -> Result<bool, PgError> {
    let n = er_entities::schedule::delete::delete(client, org_id, schedule_id).await?;
    Ok(n == 1)
}

/// A webhook row for the API listing (`webhooks.py`); the `secret` is never
/// listed (returned once on create, like an API key).
pub struct WebhookRow {
    pub id: String,
    pub url: String,
    pub events: serde_json::Value,
    pub active: bool,
}

/// Register an outbound webhook (`webhooks.py` `create`); returns its id. The
/// `secret` is the caller-chosen or generated HMAC signing key stored for
/// delivery-time signatures.
///
/// # Errors
/// Propagates the insert error.
pub async fn create_webhook(
    client: &Client,
    org_id: &str,
    url: &str,
    events: &serde_json::Value,
    secret: &str,
) -> Result<String, PgError> {
    let webhook_id = er_ids::new_v7();
    er_entities::webhook::create::create(client, &webhook_id, org_id, url, events, secret).await?;
    Ok(webhook_id)
}

/// An org's active webhooks, oldest first (`webhooks.py` `list`).
///
/// # Errors
/// Propagates the query error.
pub async fn list_webhooks(client: &Client, org_id: &str) -> Result<Vec<WebhookRow>, PgError> {
    Ok(er_entities::webhook::read::list_active(client, org_id)
        .await?
        .into_iter()
        .map(|w| WebhookRow {
            id: w.id,
            url: w.url,
            events: w.events,
            active: w.active,
        })
        .collect())
}

/// The delivery targets for one event: `(webhook_id, url, secret)` of every
/// active webhook subscribed to `event_type` (its `events` contains the type,
/// or is empty = subscribe-to-all). Unlike [`list_webhooks`], this DOES return
/// the secret — it's the HMAC key the delivery signs with.
///
/// # Errors
/// Propagates the query error.
pub async fn webhooks_for_delivery(
    client: &Client,
    org_id: &str,
    event_type: &str,
) -> Result<Vec<(String, String, String)>, PgError> {
    Ok(
        er_entities::webhook::read::for_delivery(client, org_id, event_type)
            .await?
            .into_iter()
            .map(|w| (w.id, w.url, w.secret))
            .collect(),
    )
}

/// One subscriber's `(url, secret)`, or `None` if it has been deleted.
///
/// Read at DELIVERY time rather than frozen into the queued row, so rotating a
/// secret or correcting a URL takes effect on the next attempt.
///
/// # Errors
/// Propagates the read error.
pub async fn webhook_endpoint(
    client: &Client,
    webhook_id: &str,
) -> Result<Option<(String, String)>, PgError> {
    Ok(er_entities::webhook::read::by_id(client, webhook_id)
        .await?
        .into_iter()
        .next()
        .map(|w| (w.url, w.secret)))
}

/// Delete a webhook (`webhooks.py` `delete`). `true` iff a row was removed.
///
/// # Errors
/// Propagates the delete error.
pub async fn delete_webhook(
    client: &Client,
    org_id: &str,
    webhook_id: &str,
) -> Result<bool, PgError> {
    let n = er_entities::webhook::delete::delete(client, org_id, webhook_id).await?;
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
    org_id: &str,
    org_name: &str,
    version: i64,
    tier: Option<&str>,
) -> Result<(), PgError> {
    er_entities::config_version::update::publish(client, tier, org_id, version).await?;
    er_entities::org::update::set_active_config(client, version, org_name).await?;
    Ok(())
}
