//! `er-server` — the Rust control plane (P3), an axum port of the Python
//! `erserver` FastAPI app (`server/src/erserver/api.py`). It replaces FastAPI as
//! the orgs / jobs / config / read-path HTTP surface.
//!
//! STATUS: the HTTP skeleton + the job lifecycle, over EITHER an in-memory store
//! or the real Postgres SKIP-LOCKED queue in [`pg`] — chosen per [`AppState`] by
//! whether a DSN is set (`ER_PG_DSN`). The router, `/healthz` (the only anonymous
//! surface, infrastructure.md §4.1), the bearer-token auth boundary, and the
//! `/v1/orgs/{org}/jobs` CRUD (enqueue/list/get/cancel) all work; the handlers
//! connect to Postgres per-request (no pool), mirroring the Python
//! `db.connect(dsn)` model. The in-memory path backs the network-free unit tests
//! (`tests/http.rs`); the Postgres path is exercised by the gated
//! `tests/pg_http.rs` (`ER_TEST_PG_DSN`).
//!
//! AUTH ([`Principal`] / [`Role`] / `guard`): every `/v1` route is behind a
//! bearer boundary resolving either the static operator token or an
//! `erk_{key_id}_{secret}` API key (SHA-256-hashed at rest, never argon2 —
//! matching the Python contract). The role lattice viewer<steward<admin<operator
//! and the 404-on-cross-org-scope / 403-on-insufficient-role rules are ported
//! from `auth.py`. Keys are minted/revoked at `/v1/orgs/{org}/api-keys`
//! (operator-only); gated by `tests/pg_auth.rs`. OIDC BFF auth + the
//! org-registry/config/webhook/import endpoints are still stubbed (501, TODO):
//! OIDC needs an external provider; the rest land in the next bricks.

pub mod pg;

use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::extract::{FromRequestParts, Path, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A queued unit of pipeline work (S-queue). `state` walks
/// queued -> running -> {succeeded, failed, canceled}.
#[derive(Clone, Debug, Serialize)]
pub struct Job {
    pub job_id: String,
    pub org: String,
    pub kind: String,
    pub state: String,
    pub priority: i64,
}

/// The job queue behind the API. In-memory here; the production impl is the
/// Postgres SKIP-LOCKED queue (`erserver/queue.py`), a drop-in backend swap.
#[derive(Default)]
struct JobStore {
    next: u64,
    jobs: Vec<Job>,
}

/// Process-wide server state. The OIDC verifier will join here when its
/// infrastructure lands.
///
/// The job-queue backend is chosen by `dsn`: `None` uses the in-memory
/// `JobStore` (the HTTP unit tests + the local dev default); `Some(dsn)` routes
/// the job handlers to the Postgres SKIP-LOCKED queue in [`pg`], connecting
/// per-request (mirroring the Python `db.connect(dsn)`-per-request model — no
/// pool dependency).
#[derive(Clone)]
pub struct AppState {
    /// The operator bearer token (cross-org admin). Empty disables operator auth.
    pub operator_token: String,
    store: Arc<Mutex<JobStore>>,
    dsn: Option<String>,
}

impl AppState {
    /// A state with the given operator bearer and an empty in-memory job queue.
    #[must_use]
    pub fn new(operator_token: String) -> Self {
        AppState {
            operator_token,
            store: Arc::new(Mutex::new(JobStore::default())),
            dsn: None,
        }
    }

    /// A state whose job handlers run against the Postgres control DB at `dsn`
    /// (connect-per-request). The caller is responsible for having applied the
    /// schema ([`pg::ensure_schema`]) at boot.
    #[must_use]
    pub fn with_pg(operator_token: String, dsn: String) -> Self {
        AppState {
            operator_token,
            store: Arc::new(Mutex::new(JobStore::default())),
            dsn: Some(dsn),
        }
    }
}

/// Connect to the control DB for one request, mapping a failure to a 500. The
/// error response is boxed to keep the `Result` small (`clippy::result_large_err`).
async fn connect_pg(dsn: &str) -> Result<tokio_postgres::Client, Box<axum::response::Response>> {
    pg::connect(dsn).await.map_err(|e| {
        Box::new(
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("db connect: {e}") })),
            )
                .into_response(),
        )
    })
}

/// Map a Postgres query error to a 500 response.
fn pg_err(e: tokio_postgres::Error) -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": format!("db: {e}") })),
    )
        .into_response()
}

/// The role lattice (`auth.py` `ROLE_RANK`): the derived `Ord` ranks
/// `Viewer < Steward < Admin < Operator`, so a role check is `role >= minimum`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Viewer,
    Steward,
    Admin,
    Operator,
}

impl Role {
    #[must_use]
    fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Steward => "steward",
            Role::Admin => "admin",
            Role::Operator => "operator",
        }
    }

    /// Parse a role as stored in the DB / resolved onto a principal.
    fn parse(s: &str) -> Option<Role> {
        match s {
            "viewer" => Some(Role::Viewer),
            "steward" => Some(Role::Steward),
            "admin" => Some(Role::Admin),
            "operator" => Some(Role::Operator),
            _ => None,
        }
    }

    /// Parse a role valid for an API key — operator is NOT mintable as a key
    /// (`api_keys.role` CHECK is admin/steward/viewer).
    fn parse_key_role(s: &str) -> Option<Role> {
        match Role::parse(s)? {
            Role::Operator => None,
            r => Some(r),
        }
    }
}

/// Constant-time byte equality (stands in for Python's `hmac.compare_digest`):
/// compares the full length regardless of where the first mismatch is. Lengths
/// here are fixed (operator token / 64-hex digest), so a length check leaks
/// nothing.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// An authenticated caller (`auth.py` `Principal`): the static operator token
/// resolves to `{actor: "operator", org: None, role: Operator}`; an
/// `erk_{key_id}_{secret}` API key resolves to that key's org + role. Auth
/// failures are a generic 401; authorization (scope / role) is a later `guard`.
pub struct Principal {
    pub actor: String,
    pub org: Option<String>,
    pub role: Role,
}

/// A 401 with the given generic message (auth failures never leak specifics).
fn unauthorized(msg: &str) -> (StatusCode, Json<Value>) {
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": msg })))
}

impl FromRequestParts<AppState> for Principal {
    type Rejection = (StatusCode, Json<Value>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let credential = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .map(str::trim);
        let Some(credential) = credential.filter(|c| !c.is_empty()) else {
            return Err(unauthorized("missing bearer credential"));
        };

        // The static operator token (constant-time), no DB touched.
        if !state.operator_token.is_empty()
            && ct_eq(credential.as_bytes(), state.operator_token.as_bytes())
        {
            return Ok(Principal {
                actor: "operator".to_string(),
                org: None,
                role: Role::Operator,
            });
        }

        // Otherwise an API key: erk_<key_id>_<secret> (secret may contain '_',
        // so split at most twice, like Python's `split("_", 2)`).
        let parts3: Vec<&str> = credential.splitn(3, '_').collect();
        if parts3.len() != 3 || parts3[0] != "erk" {
            return Err(unauthorized("invalid credential"));
        }
        let (key_id, secret) = (parts3[1], parts3[2]);

        // Key auth requires the Postgres backend; without it there are no keys.
        let Some(dsn) = &state.dsn else {
            return Err(unauthorized("invalid credential"));
        };
        let client = pg::connect(dsn).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("db connect: {e}") })),
            )
        })?;
        match pg::lookup_key(&client, key_id).await {
            Ok(Some((org, role, key_hash)))
                if ct_eq(key_hash.as_bytes(), pg::hash_secret(secret).as_bytes()) =>
            {
                Ok(Principal {
                    actor: format!("key:{key_id}"),
                    org: Some(org),
                    role: Role::parse(&role).unwrap_or(Role::Viewer),
                })
            }
            Ok(_) => Err(unauthorized("invalid credential")),
            Err(e) => Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("db: {e}") })),
            )),
        }
    }
}

/// Authorize an org-scoped request (`auth.py` `Principal.require`): operator
/// bypasses; otherwise a principal whose org differs gets a 404 (scope errors
/// never enumerate orgs), and an in-scope principal below `minimum` gets a 403.
fn guard(p: &Principal, org: &str, minimum: Role) -> Result<(), (StatusCode, Json<Value>)> {
    if p.role == Role::Operator {
        return Ok(());
    }
    if p.org.as_deref() != Some(org) {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("no org '{org}'") })),
        ));
    }
    if p.role < minimum {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": format!("requires {} role", minimum.as_str()) })),
        ));
    }
    Ok(())
}

/// Authorize an operator-only global surface (`auth.py` `operator_only`): plain
/// 403 for any non-operator (no org scoping involved).
fn operator_only(p: &Principal) -> Result<(), (StatusCode, Json<Value>)> {
    if p.role == Role::Operator {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "requires operator role" })),
        ))
    }
}

/// The anonymous liveness surface (§4.1): the only route that never authenticates.
async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

/// Org-name rule from `OrgIn` (`api.py`): 1..=128 chars, first `[a-z0-9]`, rest
/// `[a-z0-9_-]`.
fn valid_org_name(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes.len() > 128 {
        return false;
    }
    let is_tail = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-';
    let head_ok = bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit();
    head_ok && bytes.iter().all(|&b| is_tail(b))
}

#[derive(Deserialize)]
struct OrgIn {
    name: String,
    #[serde(default)]
    config_path: Option<String>,
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    drop_root: Option<String>,
}

/// Operator: create an org (`POST /v1/orgs`). The bring-your-own-config path
/// (a `config_path` is given) upserts the org `active` and returns `{name}`.
/// The auto-provisioning path (no `config_path`) needs the engine seed template
/// and dispatcher, which aren't wired in this environment, so it reports the
/// faithful 503 that Python raises as `ProvisioningNotConfigured`.
async fn create_org(
    principal: Principal,
    State(state): State<AppState>,
    Json(body): Json<OrgIn>,
) -> impl IntoResponse {
    if let Err(e) = operator_only(&principal) {
        return e.into_response();
    }
    if !valid_org_name(&body.name) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": format!("invalid org name '{}'", body.name) })),
        )
            .into_response();
    }
    let Some(dsn) = &state.dsn else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({ "error": "org registry requires Postgres" })),
        )
            .into_response();
    };
    let Some(config_path) = body.config_path.as_deref() else {
        // Auto mode: no seed template configured here → the real 503 branch.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "auto-provisioning not configured" })),
        )
            .into_response();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let env = json!(body.env);
    match pg::upsert_org_byo(
        &client,
        &body.name,
        config_path,
        &env,
        body.drop_root.as_deref(),
    )
    .await
    {
        Ok(()) => (StatusCode::CREATED, Json(json!({ "name": body.name }))).into_response(),
        Err(e) => pg_err(e),
    }
}

/// Operator: list all orgs (`GET /v1/orgs`).
async fn list_orgs(principal: Principal, State(state): State<AppState>) -> impl IntoResponse {
    if let Err(e) = operator_only(&principal) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({ "error": "org registry requires Postgres" })),
        )
            .into_response();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    match pg::list_orgs(&client).await {
        Ok(orgs) => {
            let out: Vec<Value> = orgs
                .iter()
                .map(|o| {
                    json!({
                        "name": o.name,
                        "state": o.state,
                        "active_config_version": o.active_config_version,
                        "created_at": o.created_at,
                    })
                })
                .collect();
            Json(json!(out)).into_response()
        }
        Err(e) => pg_err(e),
    }
}

/// Project the `resources` view out of an org's `env` map (`api.py`): the
/// DuckDB thread/memory knobs, each `null` when unset.
fn resources_of(env: &Value) -> Value {
    let threads = env
        .get("ER_DUCKDB_THREADS")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<i64>().ok());
    let memory = env.get("ER_DUCKDB_MEMORY_LIMIT").and_then(Value::as_str);
    json!({ "duckdb_threads": threads, "duckdb_memory_limit": memory })
}

/// Viewer+: an org's detail (`GET /v1/orgs/{org}`), 404 if absent.
async fn get_org(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({ "error": "org registry requires Postgres" })),
        )
            .into_response();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    match pg::get_org(&client, &org).await {
        Ok(Some(o)) => Json(json!({
            "name": o.name,
            "state": o.state,
            "active_config_version": o.active_config_version,
            "drop_root": o.drop_root,
            "resources": resources_of(&o.env),
        }))
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("no org '{org}'") })),
        )
            .into_response(),
        Err(e) => pg_err(e),
    }
}

/// Transition an org's lifecycle state under an operator guard (the shared body
/// of suspend/resume): 404 if absent, 409 if not in `expected`.
async fn transition_org(
    principal: &Principal,
    state: &AppState,
    org: &str,
    new_state: &str,
    expected: &str,
    not_in_expected: &str,
) -> axum::response::Response {
    if let Err(e) = guard(principal, org, Role::Operator) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({ "error": "org registry requires Postgres" })),
        )
            .into_response();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    match pg::org_exists(&client, org).await {
        Ok(false) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("no org '{org}'") })),
            )
                .into_response();
        }
        Err(e) => return pg_err(e),
        Ok(true) => {}
    }
    match pg::set_org_state(&client, org, new_state, expected).await {
        Ok(true) => (
            StatusCode::OK,
            Json(json!({ "name": org, "state": new_state })),
        )
            .into_response(),
        Ok(false) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": not_in_expected })),
        )
            .into_response(),
        Err(e) => pg_err(e),
    }
}

/// Operator: suspend an active org (`POST /v1/orgs/{org}/suspend`; the Python
/// `{org}:suspend` action — a `/suspend` sub-path, per the matchit constraint).
async fn suspend_org(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
) -> impl IntoResponse {
    let msg = format!("org '{org}' is not active; only active orgs suspend");
    transition_org(&principal, &state, &org, "suspended", "active", &msg).await
}

/// Operator: resume a suspended org (`POST /v1/orgs/{org}/resume`).
async fn resume_org(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
) -> impl IntoResponse {
    let msg = format!("org '{org}' is not suspended; nothing to resume");
    transition_org(&principal, &state, &org, "active", "suspended", &msg).await
}

#[derive(Deserialize)]
struct EnqueueJob {
    kind: String,
    #[serde(default)]
    priority: i64,
}

/// Enqueue a job for an org (S-queue enqueue). Requires steward+ in the org.
async fn create_job(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<EnqueueJob>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    if body.kind.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "kind required" })),
        )
            .into_response();
    }
    if let Some(dsn) = &state.dsn {
        let client = match connect_pg(dsn).await {
            Ok(c) => c,
            Err(r) => return *r,
        };
        let priority = i32::try_from(body.priority).unwrap_or(0);
        return match pg::enqueue(&client, &org, &body.kind, priority, None).await {
            Ok(job) => (StatusCode::ACCEPTED, Json(json!(job))).into_response(),
            Err(e) => pg_err(e),
        };
    }
    let job = enqueue(&state, &org, &body.kind, body.priority);
    (StatusCode::ACCEPTED, Json(json!(job))).into_response()
}

/// Enqueue a `queued` job directly (the body of `create_job`, reusable by the
/// dispatcher and tests). Returns the minted job.
pub fn enqueue(state: &AppState, org: &str, kind: &str, priority: i64) -> Job {
    let mut store = state.store.lock().unwrap();
    let job_id = format!("job-{}", store.next);
    store.next += 1;
    let job = Job {
        job_id,
        org: org.to_string(),
        kind: kind.to_string(),
        state: "queued".to_string(),
        priority,
    };
    store.jobs.push(job.clone());
    job
}

/// Dispatch one job (the control-plane -> data-plane bridge, S-queue claim):
/// claim the highest-priority `queued` job (ties broken by enqueue order, like
/// the Postgres `ORDER BY priority DESC, job_id`), mark it `running`, run it via
/// `runner`, then record `succeeded`/`failed`. Returns the job in its final
/// state, or `None` if nothing was queued.
///
/// `runner` is injected so the queue mechanics stay backend-agnostic: the binary
/// wires a runner that opens the job's DuckLake and invokes `er_cli::run_all`;
/// the in-memory store here becomes the Postgres SKIP-LOCKED queue later.
pub fn dispatch_next(
    state: &AppState,
    runner: impl FnOnce(&Job) -> Result<(), String>,
) -> Option<Job> {
    let job = {
        let mut store = state.store.lock().unwrap();
        let idx = store
            .jobs
            .iter()
            .enumerate()
            .filter(|(_, j)| j.state == "queued")
            .max_by(|a, b| {
                a.1.priority.cmp(&b.1.priority).then_with(|| b.0.cmp(&a.0)) // earlier index wins the tie
            })
            .map(|(i, _)| i)?;
        store.jobs[idx].state = "running".to_string();
        store.jobs[idx].clone()
    };

    let outcome = runner(&job);

    let mut store = state.store.lock().unwrap();
    let slot = store
        .jobs
        .iter_mut()
        .find(|j| j.job_id == job.job_id)
        .expect("claimed job still present");
    slot.state = if outcome.is_ok() {
        "succeeded"
    } else {
        "failed"
    }
    .to_string();
    Some(slot.clone())
}

/// List an org's jobs, newest first. Requires viewer+ in the org.
async fn list_jobs(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
) -> axum::response::Response {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    if let Some(dsn) = &state.dsn {
        let client = match connect_pg(dsn).await {
            Ok(c) => c,
            Err(r) => return *r,
        };
        return match pg::list_jobs(&client, &org).await {
            Ok(jobs) => Json(json!({ "jobs": jobs })).into_response(),
            Err(e) => pg_err(e),
        };
    }
    let store = state.store.lock().unwrap();
    let jobs: Vec<&Job> = store.jobs.iter().rev().filter(|j| j.org == org).collect();
    Json(json!({ "jobs": jobs })).into_response()
}

/// One job by id (404 if absent / not this org's). Requires viewer+ in the org.
async fn get_job(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, job_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    if let Some(dsn) = &state.dsn {
        let client = match connect_pg(dsn).await {
            Ok(c) => c,
            Err(r) => return *r,
        };
        return match pg::get_job(&client, &org, &job_id).await {
            Ok(Some(job)) => (StatusCode::OK, Json(json!(job))).into_response(),
            Ok(None) => (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "no such job" })),
            )
                .into_response(),
            Err(e) => pg_err(e),
        };
    }
    let store = state.store.lock().unwrap();
    match store
        .jobs
        .iter()
        .find(|j| j.org == org && j.job_id == job_id)
    {
        Some(job) => (StatusCode::OK, Json(json!(job))).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "no such job" })),
        )
            .into_response(),
    }
}

/// Cancel a job (idempotent-ish: queued/running -> canceled). Requires steward+.
async fn cancel_job(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, job_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    if let Some(dsn) = &state.dsn {
        let client = match connect_pg(dsn).await {
            Ok(c) => c,
            Err(r) => return *r,
        };
        // Scope the cancel to the org (404 if absent / another org's), then mark.
        return match pg::get_job(&client, &org, &job_id).await {
            Ok(Some(job)) => match pg::mark(&client, &job_id, "canceled").await {
                Ok(()) => (
                    StatusCode::OK,
                    Json(json!(Job {
                        state: "canceled".to_string(),
                        ..job
                    })),
                )
                    .into_response(),
                Err(e) => pg_err(e),
            },
            Ok(None) => (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "no such job" })),
            )
                .into_response(),
            Err(e) => pg_err(e),
        };
    }
    let mut store = state.store.lock().unwrap();
    match store
        .jobs
        .iter_mut()
        .find(|j| j.org == org && j.job_id == job_id)
    {
        Some(job) => {
            job.state = "canceled".to_string();
            (StatusCode::OK, Json(json!(job.clone()))).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "no such job" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct KeyIn {
    role: String,
}

/// Operator: mint an API key for an org, returning the plaintext ONCE
/// (`POST /v1/orgs/{org}/api-keys`). Needs the Postgres backend.
async fn create_key(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<KeyIn>,
) -> impl IntoResponse {
    if let Err(e) = operator_only(&principal) {
        return e.into_response();
    }
    // Operator can mint for any org; a key role of operator is rejected (422).
    let Some(role) = Role::parse_key_role(&body.role) else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": format!("invalid role '{}'", body.role) })),
        )
            .into_response();
    };
    let Some(dsn) = &state.dsn else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({ "error": "api keys require Postgres" })),
        )
            .into_response();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    match pg::issue_key(&client, &org, role.as_str()).await {
        Ok((key_id, key)) => (
            StatusCode::CREATED,
            Json(json!({ "key_id": key_id, "key": key, "role": role.as_str() })),
        )
            .into_response(),
        Err(e) => pg_err(e),
    }
}

/// Operator: revoke an API key, org-scoped and idempotent
/// (`DELETE /v1/orgs/{org}/api-keys/{key_id}`).
async fn revoke_key(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, key_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = operator_only(&principal) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({ "error": "api keys require Postgres" })),
        )
            .into_response();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    match pg::revoke_key(&client, &key_id, &org).await {
        Ok(revoked) => (StatusCode::OK, Json(json!({ "revoked": revoked }))).into_response(),
        Err(e) => pg_err(e),
    }
}

/// Build the control-plane router. `/healthz` is anonymous; every `/v1` route is
/// behind the bearer boundary.
pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/orgs", get(list_orgs).post(create_org))
        .route("/v1/orgs/{org}", get(get_org))
        // Python's `{org}:suspend` / `{org}:resume` actions become sub-paths
        // (matchit forbids a param+literal in one segment, as with job cancel).
        .route("/v1/orgs/{org}/suspend", post(suspend_org))
        .route("/v1/orgs/{org}/resume", post(resume_org))
        .route("/v1/orgs/{org}/jobs", post(create_job).get(list_jobs))
        .route("/v1/orgs/{org}/jobs/{job_id}", get(get_job))
        // axum/matchit requires a param to be a whole segment, so the Python
        // `{job_id}:cancel` action becomes a `/cancel` sub-path here.
        .route("/v1/orgs/{org}/jobs/{job_id}/cancel", post(cancel_job))
        .route("/v1/orgs/{org}/api-keys", post(create_key))
        .route(
            "/v1/orgs/{org}/api-keys/{key_id}",
            axum::routing::delete(revoke_key),
        )
        .with_state(state)
}

/// Build the server state from the environment: the operator bearer
/// (`ER_OPERATOR_TOKEN`) and, if `ER_PG_DSN` is set, the Postgres job-queue
/// backend (otherwise the in-memory store).
#[must_use]
pub fn state_from_env() -> AppState {
    let operator = std::env::var("ER_OPERATOR_TOKEN").unwrap_or_default();
    match std::env::var("ER_PG_DSN") {
        Ok(dsn) if !dsn.is_empty() => AppState::with_pg(operator, dsn),
        _ => AppState::new(operator),
    }
}
