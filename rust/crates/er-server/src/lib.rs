//! `er-server` — the Rust control plane (P3), an axum port of the Python
//! `erserver` FastAPI app (`server/src/erserver/api.py`). It replaces FastAPI as
//! the orgs / jobs / config / read-path HTTP surface.
//!
//! STATUS. The control plane runs over EITHER an in-memory store or the real
//! Postgres SKIP-LOCKED queue in [`pg`] — chosen per [`AppState`] by whether a
//! DSN is set (`ER_PG_DSN`); handlers connect to Postgres per-request (no pool),
//! mirroring the Python `db.connect(dsn)` model. Live surfaces:
//!   - orgs CRUD + lifecycle (create/list/get/suspend/resume),
//!   - jobs (enqueue/list/get/cancel) over the SKIP-LOCKED queue, and the real
//!     [`dispatch_pg`] bridge (claim → runner → mark),
//!   - config versions (create/list/get/active/publish, with tier classification),
//!   - the DuckLake-backed read API in [`read`] (golden-records list/detail,
//!     metrics, duplicates, match-scores, records, reviews, assertions, imports,
//!     runs) — resolves each org to its lake from `orgs.env` and reads
//!     `lake.main.*` snapshot-pinned.
//!
//! AUTH ([`Principal`] / [`Role`] / `guard`): every `/v1` route is behind a
//! bearer boundary resolving either the static operator token or an
//! `erk_{key_id}_{secret}` API key (SHA-256-hashed at rest, never argon2). The
//! role lattice viewer<steward<admin<operator and the 404-on-cross-org-scope /
//! 403-on-insufficient-role rules are ported from `auth.py`. Error bodies are
//! the FastAPI `{detail}` shape the frontend BFF parses.
//!
//! NOT YET PORTED: the steward WRITE surface (assertion create/delete, review
//! resolve, unmerge, job resume) and merge-plans — they need the review /
//! assertion / run-tracking data-plane stages that were removed with the Python.
//! OIDC BFF auth and the k8s launcher are infrastructure-bound. The mailer is
//! live ([`mail`], `POST /v1/email`) but inert until an environment configures a
//! backend. Unit tests are network-free (`tests/http.rs`); the Postgres +
//! DuckLake paths are gated (`ER_TEST_PG_DSN`, `DEDUPE_E2E_DUCKLAKE`).

pub mod config_check;
pub mod dispatcher;
pub mod errfmt;
pub mod mail;
pub mod openapi;
pub mod pg;
pub mod read;
pub mod webhook;

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use er_config::{Config, config_hash, load_config_str};

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{FromRequestParts, Path, Query, Request, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Value, json};
use tower::ServiceExt as _;

/// A queued unit of pipeline work (S-queue). `state` walks
/// queued -> running -> {succeeded, failed, canceled}. Re-exported from the
/// generated entity crate so the queue, handlers and dispatcher share one
/// definition (the full `jobs` row, not a hand-picked projection).
pub use er_entities::job::Job;

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
    /// Outbound email. Defaults to [`mail::Mailer::off`], which is why the
    /// network-free unit tests can construct a state without configuring one:
    /// `POST /v1/email` then answers 503, exactly as an environment with no mail
    /// backend does.
    mail: Arc<mail::Mailer>,
}

impl AppState {
    /// A state with the given operator bearer and an empty in-memory job queue.
    #[must_use]
    pub fn new(operator_token: String) -> Self {
        AppState {
            operator_token,
            store: Arc::new(Mutex::new(JobStore::default())),
            dsn: None,
            mail: Arc::new(mail::Mailer::off()),
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
            mail: Arc::new(mail::Mailer::off()),
        }
    }

    /// Attach a mail backend. Separate from the constructors because building a
    /// SES client is async and these are not.
    #[must_use]
    pub fn with_mail(mut self, mailer: mail::Mailer) -> Self {
        self.mail = Arc::new(mailer);
        self
    }
}

/// Connect to the control DB for one request, mapping a failure to a 500. The
/// error response is boxed to keep the `Result` small (`clippy::result_large_err`).
async fn connect_pg(dsn: &str) -> Result<tokio_postgres::Client, Box<axum::response::Response>> {
    pg::connect(dsn).await.map_err(|e| {
        Box::new(
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "detail": format!("db connect: {e}") })),
            )
                .into_response(),
        )
    })
}

/// Map a Postgres query error to a 500 response.
/// Resolve the path org (its NAME — the routing handle) to its row, or the
/// standard scope-safe 404. Child tables carry `org_id` (the org's UUIDv7), so
/// every org-scoped handler resolves once and threads `row.id` to the queries.
async fn org_or_404(
    client: &tokio_postgres::Client,
    org: &str,
) -> Result<pg::OrgDetail, Box<axum::response::Response>> {
    match pg::get_org(client, org).await {
        Ok(Some(row)) => Ok(row),
        Ok(None) => Err(Box::new(
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "detail": format!("no org '{org}'") })),
            )
                .into_response(),
        )),
        Err(e) => Err(Box::new(pg_err(e))),
    }
}

fn pg_err(e: tokio_postgres::Error) -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "detail": format!("db: {e}") })),
    )
        .into_response()
}

/// A `{ "detail": … }` error body — the FastAPI shape the frontend BFF parses
/// (`proxy.ts` reads `detail`, and keys its user-facing mapping on the status).
fn detail_err(code: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (code, Json(json!({ "detail": msg.into() }))).into_response()
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
    (StatusCode::UNAUTHORIZED, Json(json!({ "detail": msg })))
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
                Json(json!({ "detail": format!("db connect: {e}") })),
            )
        })?;
        match pg::lookup_key(&client, key_id).await {
            Ok(Some((org_id, role, key_hash)))
                if ct_eq(key_hash.as_bytes(), pg::hash_secret(secret).as_bytes()) =>
            {
                // Principal.org carries the org NAME (the routing handle every
                // guard compares against); keys store the org's id.
                let org_name = match pg::get_org_by_id(&client, &org_id).await {
                    Ok(Some(o)) => o.name,
                    Ok(None) => return Err(unauthorized("invalid credential")),
                    Err(e) => {
                        return Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({ "detail": format!("db: {e}") })),
                        ));
                    }
                };
                Ok(Principal {
                    actor: format!("key:{key_id}"),
                    org: Some(org_name),
                    role: Role::parse(&role).unwrap_or(Role::Viewer),
                })
            }
            Ok(_) => Err(unauthorized("invalid credential")),
            Err(e) => Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "detail": format!("db: {e}") })),
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
            Json(json!({ "detail": format!("no org '{org}'") })),
        ));
    }
    if p.role < minimum {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "detail": format!("requires {} role", minimum.as_str()) })),
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
            Json(json!({ "detail": "requires operator role" })),
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
            Json(json!({ "detail": format!("invalid org name '{}'", body.name) })),
        )
            .into_response();
    }
    let Some(dsn) = &state.dsn else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({ "detail": "org registry requires Postgres" })),
        )
            .into_response();
    };
    let Some(config_path) = body.config_path.as_deref() else {
        // Auto mode: no seed template configured here → the real 503 branch.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "detail": "auto-provisioning not configured" })),
        )
            .into_response();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let env = json!(body.env);
    // History images record the acting principal (er.actor).
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::upsert_org_byo(
        &client,
        &body.name,
        config_path,
        &env,
        body.drop_root.as_deref(),
    )
    .await
    {
        Ok(()) => {
            let _ = pg::audit(
                &client,
                &principal.actor,
                Some(&body.name),
                "org.upsert",
                &json!({}),
            )
            .await;
            (StatusCode::CREATED, Json(json!({ "name": body.name }))).into_response()
        }
        Err(e) => pg_err(e),
    }
}

/// Operator: send one transactional email (`POST /v1/email`).
///
/// The BFF's only mail path (`frontend/src/lib/bff/relay.ts`), so the platform
/// keeps one transport and one template set. Operator-only because the BFF holds
/// the operator token and nothing else should be able to make this service send
/// mail to an arbitrary address.
///
/// The status codes matter to the caller, which treats failure as data:
/// **422** a template or parameter the caller got wrong — a real bug, fix the
/// call site; **503** this environment has no mail backend, which is the normal
/// state in dev and the case `relayEmail` turns into `false`; **502** the
/// transport rejected it. On success **202**: SES has accepted the message, and
/// delivery is its problem from there.
async fn send_email(
    principal: Principal,
    State(state): State<AppState>,
    Json(body): Json<mail::Message>,
) -> impl IntoResponse {
    if let Err(e) = operator_only(&principal) {
        return e.into_response();
    }
    if body.to.trim().is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "detail": "a recipient is required" })),
        )
            .into_response();
    }
    match state.mail.send(&body).await {
        Ok(()) => (StatusCode::ACCEPTED, Json(json!({}))).into_response(),
        Err(e @ mail::MailError::Invalid(_)) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "detail": e.to_string() })),
        )
            .into_response(),
        Err(e @ mail::MailError::NotConfigured) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "detail": e.to_string() })),
        )
            .into_response(),
        Err(e @ mail::MailError::Transport { .. }) => {
            // The recipient is deliberately absent from the log line: this runs
            // for password resets, so the address is the one thing worth not
            // writing down on every transport hiccup.
            eprintln!(
                "er-server: email transport failed (template={}): {}",
                body.template,
                errfmt::chain(&e)
            );
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "detail": "the mail transport rejected the message" })),
            )
                .into_response()
        }
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
            Json(json!({ "detail": "org registry requires Postgres" })),
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
                        "id": o.id,
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
            Json(json!({ "detail": "org registry requires Postgres" })),
        )
            .into_response();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    match pg::get_org(&client, &org).await {
        Ok(Some(o)) => Json(json!({
            "id": o.id,
            "name": o.name,
            "state": o.state,
            "active_config_version": o.active_config_version,
            "drop_root": o.drop_root,
            "resources": resources_of(&o.env),
        }))
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "detail": format!("no org '{org}'") })),
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
            Json(json!({ "detail": "org registry requires Postgres" })),
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
                Json(json!({ "detail": format!("no org '{org}'") })),
            )
                .into_response();
        }
        Err(e) => return pg_err(e),
        Ok(true) => {}
    }
    // History images record the acting principal (er.actor).
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::set_org_state(&client, org, new_state, expected).await {
        Ok(true) => {
            let action = if new_state == "suspended" {
                "org.suspend"
            } else {
                "org.resume"
            };
            let _ = pg::audit(&client, &principal.actor, Some(org), action, &json!({})).await;
            (
                StatusCode::OK,
                Json(json!({ "name": org, "state": new_state })),
            )
                .into_response()
        }
        Ok(false) => (
            StatusCode::CONFLICT,
            Json(json!({ "detail": not_in_expected })),
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
struct ConfigIn {
    yaml: String,
}

/// 422 for an invalid config (`api.py`: `ConfigValidationError`). `er-config`'s
/// error renders a YAML line/col location, not a JSON pointer.
fn config_invalid(e: &er_config::ConfigError) -> (StatusCode, Json<Value>) {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "detail": format!("config error: {e}") })),
    )
}

/// Admin+: create a draft config version (`POST /v1/orgs/{org}/config/versions`).
/// The YAML is structurally validated and hashed; the version is `max+1`.
async fn create_config_version(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<ConfigIn>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Admin) {
        return e.into_response();
    }
    let config = match load_config_str(&body.yaml) {
        Ok(c) => c,
        Err(e) => return config_invalid(&e).into_response(),
    };
    let hash = config_hash(&config);
    let Some(dsn) = &state.dsn else {
        return config_registry_unavailable();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    // History images record the acting principal (er.actor).
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::create_config_version(&client, &org_row.id, &body.yaml, &hash, &principal.actor).await
    {
        Ok(version) => (
            StatusCode::CREATED,
            Json(json!({
                "org": org,
                "version": version,
                "config_hash": hash,
                "state": "draft",
            })),
        )
            .into_response(),
        Err(e) => pg_err(e),
    }
}

/// The 501 for config routes when no Postgres backend is configured.
fn config_registry_unavailable() -> axum::response::Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({ "detail": "config registry requires Postgres" })),
    )
        .into_response()
}

/// Viewer+: list an org's config versions, newest first (no `yaml` body).
async fn list_config_versions(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return config_registry_unavailable();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    match pg::list_config_versions(&client, &org_row.id).await {
        Ok(rows) => {
            let out: Vec<Value> = rows
                .iter()
                .map(|r| {
                    json!({
                        "version": r.version,
                        "config_hash": r.config_hash,
                        "state": r.state,
                        "tier": r.tier,
                        "created_by": r.created_by,
                        "created_at": r.created_at,
                        "published_at": r.published_at,
                    })
                })
                .collect();
            Json(json!(out)).into_response()
        }
        Err(e) => pg_err(e),
    }
}

/// Parse a `{version}` path segment to the stored integer, or `None`.
fn parse_version(s: &str) -> Option<i64> {
    s.parse::<i64>().ok()
}

/// Viewer+: one config version with its `yaml` (404 if absent).
async fn get_config_version(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, version)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(version) = parse_version(&version) else {
        return config_version_missing(&org, &version).into_response();
    };
    let Some(dsn) = &state.dsn else {
        return config_registry_unavailable();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    match pg::get_config_version(&client, &org_row.id, version).await {
        Ok(Some(d)) => Json(config_detail_json(&d)).into_response(),
        Ok(None) => config_version_missing(&org, &version.to_string()).into_response(),
        Err(e) => pg_err(e),
    }
}

/// The 404 body for a missing config version.
fn config_version_missing(org: &str, version: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "detail": format!("no config version {version} for org '{org}'") })),
    )
}

/// JSON of a full config-version detail.
fn config_detail_json(d: &pg::ConfigVersionDetail) -> Value {
    json!({
        "version": d.version,
        "yaml": d.yaml,
        "config_hash": d.config_hash,
        "state": d.state,
        "tier": d.tier,
        "created_by": d.created_by,
        "created_at": d.created_at,
        "published_at": d.published_at,
    })
}

/// Viewer+: the org's active (published) config (`GET /v1/orgs/{org}/config`),
/// 404 if nothing is published.
async fn active_config(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return config_registry_unavailable();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let active = match pg::org_publish_context(&client, &org).await {
        Ok(Some((_, _, Some(v)))) => v,
        Ok(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "detail": "no published config" })),
            )
                .into_response();
        }
        Err(e) => return pg_err(e),
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    match pg::get_config_version(&client, &org_row.id, active).await {
        Ok(Some(d)) => Json(config_detail_json(&d)).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "detail": "no published config" })),
        )
            .into_response(),
        Err(e) => pg_err(e),
    }
}

// ─── Nickname/synonym lexicon editing (finishing the M6 partial) ────────────
//
// The lexicon is a per-org matching input NOT in `er-config::Config` and NOT a
// committed fixture, so editing it at runtime is determinism-safe: it persists
// to the org's own `ER_LEXICON_PATH` CSV (the file the dispatcher seeds from),
// never the packaged fixtures/goldens or any DDL. An edit takes effect on the
// next run, so a PUT enqueues a `run_all_full` (skip_ingest) rebuild — the same
// apply path config-publish uses.

/// The packaged default lexicon returned when an org has none yet (mirrors
/// er-cli's `tests/fixtures/nickname_variants.csv`).
const DEFAULT_LEXICON: [(&str, &str); 12] = [
    ("anthony", "tony"),
    ("beth", "elizabeth"),
    ("bob", "robert"),
    ("bobby", "robert"),
    ("catherine", "cathy"),
    ("chris", "christopher"),
    ("dave", "david"),
    ("elizabeth", "liz"),
    ("james", "jim"),
    ("james", "jimmy"),
    ("michael", "mike"),
    ("sue", "susan"),
];

#[derive(serde::Deserialize)]
struct LexiconPair {
    variant_a: String,
    variant_b: String,
}

#[derive(serde::Deserialize)]
struct LexiconPut {
    pairs: Vec<LexiconPair>,
}

/// Read a lexicon CSV (`variant_a,variant_b` with a header row) into pairs, as
/// `load_lexicon`/the dispatcher do. Missing file → empty.
fn read_lexicon_pairs(path: &str) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            l.split_once(',')
                .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
        })
        .collect()
}

fn lexicon_pairs_json(pairs: &[(String, String)]) -> Vec<Value> {
    pairs
        .iter()
        .map(|(a, b)| json!({ "variant_a": a, "variant_b": b }))
        .collect()
}

/// A sibling file next to `path` (same directory), used to derive an
/// `ER_LEXICON_PATH` from the org's `ER_CONFIG_PATH` when none is set yet.
fn sibling_path(path: &str, name: &str) -> String {
    std::path::Path::new(path).parent().map_or_else(
        || name.to_string(),
        |d| d.join(name).to_string_lossy().into_owned(),
    )
}

/// Viewer+: the org's nickname lexicon (`GET /v1/orgs/{org}/lexicon`). Returns
/// the packaged default when the org has none yet, so the editor never starts empty.
async fn get_lexicon(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return config_registry_unavailable();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match pg::get_org(&client, &org).await {
        Ok(Some(o)) => o,
        Ok(None) => return detail_err(StatusCode::NOT_FOUND, format!("no org '{org}'")),
        Err(e) => return pg_err(e),
    };
    let pairs: Vec<(String, String)> =
        match org_row.env.get("ER_LEXICON_PATH").and_then(Value::as_str) {
            Some(p) if std::path::Path::new(p).exists() => read_lexicon_pairs(p),
            _ => DEFAULT_LEXICON
                .iter()
                .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
                .collect(),
        };
    Json(json!({ "pairs": lexicon_pairs_json(&pairs) })).into_response()
}

/// Steward+: replace the org's nickname lexicon (`PUT /v1/orgs/{org}/lexicon`).
/// Canonicalises + validates each pair, atomically writes the org's
/// `ER_LEXICON_PATH` CSV (deriving + persisting one from `ER_CONFIG_PATH` if
/// unset), then enqueues a `run_all_full` rebuild to apply it.
async fn put_lexicon(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<LexiconPut>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return config_registry_unavailable();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match pg::get_org(&client, &org).await {
        Ok(Some(o)) if o.state == "active" => o,
        Ok(Some(o)) => {
            return detail_err(
                StatusCode::CONFLICT,
                format!(
                    "org '{org}' is {}; lexicon edits are accepted only when active",
                    o.state
                ),
            );
        }
        Ok(None) => return detail_err(StatusCode::NOT_FOUND, format!("no org '{org}'")),
        Err(e) => return pg_err(e),
    };

    // Validate + canonicalise. Commas/control chars would corrupt the CSV or the
    // lexicon_hash separators, so reject them before canonicalising.
    let mut set: std::collections::BTreeSet<(String, String)> = std::collections::BTreeSet::new();
    for p in &body.pairs {
        for v in [&p.variant_a, &p.variant_b] {
            if v.contains(',') || v.chars().any(char::is_control) {
                return detail_err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "variant names must not contain commas or control characters",
                );
            }
        }
        match er_std::lexicon::canonical_pair(&p.variant_a, &p.variant_b) {
            Ok(pair) => {
                set.insert(pair);
            }
            Err(e) => return detail_err(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()),
        }
    }

    // Resolve the per-org lexicon file: the existing ER_LEXICON_PATH, else a
    // sibling of ER_CONFIG_PATH (persisted into env below).
    let existing = org_row
        .env
        .get("ER_LEXICON_PATH")
        .and_then(Value::as_str)
        .map(str::to_string);
    let path = match &existing {
        Some(p) => p.clone(),
        None => match org_row.env.get("ER_CONFIG_PATH").and_then(Value::as_str) {
            Some(cfg) => sibling_path(cfg, "lexicon.csv"),
            None => {
                return detail_err(
                    StatusCode::CONFLICT,
                    "org has no ER_LEXICON_PATH, and no ER_CONFIG_PATH to derive one from",
                );
            }
        },
    };

    let mut csv = String::from("variant_a,variant_b\n");
    for (a, b) in &set {
        csv.push_str(a);
        csv.push(',');
        csv.push_str(b);
        csv.push('\n');
    }
    if let Err(e) = write_config_file(&path, &csv) {
        return detail_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("lexicon write: {e}"),
        );
    }
    if existing.is_none() {
        if let Err(e) = pg::merge_org_env(&client, &org, &json!({ "ER_LEXICON_PATH": path })).await
        {
            return pg_err(e);
        }
    }

    let idem = format!("lexicon-apply:{}", er_ids::new_v7());
    let params = json!({ "skip_ingest": true });
    let apply_job = match pg::enqueue(
        &client,
        &org_row.id,
        "run_all_full",
        0,
        Some(&idem),
        Some(&params),
    )
    .await
    {
        Ok(j) => j.id,
        Err(e) => return pg_err(e),
    };
    let _ = pg::audit(
        &client,
        &principal.actor,
        Some(&org),
        "lexicon.update",
        &json!({ "pairs": set.len() }),
    )
    .await;

    let pairs: Vec<(String, String)> = set.into_iter().collect();
    (
        StatusCode::OK,
        Json(json!({ "pairs": lexicon_pairs_json(&pairs), "apply_job": apply_job })),
    )
        .into_response()
}

/// Change tiers (`configsvc.py` `classify_tier`), biggest-blast-radius first.
const TIER_C_BLOCKS: [&str; 3] = ["comparisons", "standardization", "sources"];
const TIER_B_BLOCKS: [&str; 1] = ["blocking"];
const TIER_A_BLOCKS: [&str; 2] = ["thresholds", "survivorship"];
/// Blocks only an operator may change (`configsvc.py`): everything structural.
const OPERATOR_ONLY_BLOCKS: [&str; 7] = [
    "tenant",
    "storage",
    "versions",
    "training",
    "clustering",
    "coherence",
    "generator",
];

/// Top-level config blocks that differ between two configs, compared via their
/// canonical serde representation (the object keys are exactly the 14 S6 blocks).
fn config_changed_blocks(old: &Config, new: &Config) -> Vec<String> {
    let old_v = serde_json::to_value(old).unwrap_or(Value::Null);
    let new_v = serde_json::to_value(new).unwrap_or(Value::Null);
    let old_m = old_v.as_object();
    let new_m = new_v.as_object();
    let mut keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if let Some(m) = old_m {
        keys.extend(m.keys().cloned());
    }
    if let Some(m) = new_m {
        keys.extend(m.keys().cloned());
    }
    keys.into_iter()
        .filter(|k| old_m.and_then(|m| m.get(k)) != new_m.and_then(|m| m.get(k)))
        .collect()
}

/// The publish tier for a changed-block set: C > B > A, else `None`.
fn tier_for(changed: &[String]) -> Option<&'static str> {
    let hits = |set: &[&str]| changed.iter().any(|b| set.contains(&b.as_str()));
    if hits(&TIER_C_BLOCKS) {
        Some("C")
    } else if hits(&TIER_B_BLOCKS) {
        Some("B")
    } else if hits(&TIER_A_BLOCKS) {
        Some("A")
    } else {
        None
    }
}

/// Atomically write the published YAML to the org's `config_path` (tempfile +
/// rename), the file the dispatcher reads. Synchronous IO is acceptable here —
/// publish is rare and not on a hot path.
fn write_config_file(path: &str, yaml: &str) -> std::io::Result<()> {
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, yaml)?;
    std::fs::rename(&tmp, path)
}

/// Admin+ (operator for structural blocks): publish a draft config version
/// (`POST /v1/orgs/{org}/config/versions/{version}/publish`; the Python
/// `{version}:publish` action). Re-validates, classifies the change tier versus
/// the current active config, writes the YAML to `config_path`, points
/// `active_config_version` at it, and enqueues the tier's rebuild jobs.
///
/// Soundness note: this refuses to publish to a non-active org up front (409)
/// rather than publishing then 409-ing at enqueue as the Python does — an atomic
/// refusal avoids a half-applied publish.
async fn publish_config_version(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, version)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Admin) {
        return e.into_response();
    }
    let Some(version) = parse_version(&version) else {
        return config_version_missing(&org, &version).into_response();
    };
    let Some(dsn) = &state.dsn else {
        return config_registry_unavailable();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };

    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };

    // The version being published + its re-validated config.
    let detail = match pg::get_config_version(&client, &org_row.id, version).await {
        Ok(Some(d)) => d,
        Ok(None) => return config_version_missing(&org, &version.to_string()).into_response(),
        Err(e) => return pg_err(e),
    };
    let new_config = match load_config_str(&detail.yaml) {
        Ok(c) => c,
        Err(e) => return config_invalid(&e).into_response(),
    };

    // Publish-time analysis: the config's blocking/comparison definitions must
    // only reference columns the standardized-records table has for this entity
    // type. A mistyped or non-existent column is refused here rather than failing
    // the tenant's first run.
    let reference_problems = config_check::config_reference_problems(&new_config);
    if !reference_problems.is_empty() {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "config references columns the standardized schema does not have: {}",
                reference_problems.join("; ")
            ),
        )
        .into_response();
    }

    // Org publish context (config_path / state / current active version).
    let (config_path, org_state, active_v) = match pg::org_publish_context(&client, &org).await {
        Ok(Some(ctx)) => ctx,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "detail": format!("no org '{org}'") })),
            )
                .into_response();
        }
        Err(e) => return pg_err(e),
    };

    // Classify the change versus the current active config (first publish => C).
    let (tier, changed): (Option<&str>, Vec<String>) = match active_v {
        None => (Some("C"), Vec::new()),
        Some(av) => {
            let old_yaml = match pg::get_config_version(&client, &org_row.id, av).await {
                Ok(Some(d)) => d.yaml,
                Ok(None) => String::new(),
                Err(e) => return pg_err(e),
            };
            match load_config_str(&old_yaml) {
                Ok(old_config) => {
                    let changed = config_changed_blocks(&old_config, &new_config);
                    (tier_for(&changed), changed)
                }
                // A previously-published config that no longer parses: treat the
                // whole document as changed (fail safe to the broadest tier).
                Err(_) => (Some("C"), Vec::new()),
            }
        }
    };

    // Structural (operator-only) blocks may only be changed by the operator.
    let operator_only: Vec<&String> = changed
        .iter()
        .filter(|b| OPERATOR_ONLY_BLOCKS.contains(&b.as_str()))
        .collect();
    if !operator_only.is_empty() && principal.role != Role::Operator {
        return detail_err(
            StatusCode::FORBIDDEN,
            format!("blocks {operator_only:?} are operator-only; publish refused"),
        );
    }

    // Refuse publishing to a suspended/provisioning org atomically.
    if org_state != "active" {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "detail": format!("org '{org}' is not active") })),
        )
            .into_response();
    }

    // Write the YAML the dispatcher reads, then record the publish + repoint the
    // org, then enqueue the tier's rebuild jobs.
    if let Err(e) = write_config_file(&config_path, &detail.yaml) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "detail": format!("config write: {e}") })),
        )
            .into_response();
    }
    // History images record the acting principal (er.actor).
    let _ = pg::set_actor(&client, &principal.actor).await;
    if let Err(e) = pg::publish_config_version(&client, &org_row.id, &org, version, tier).await {
        return pg_err(e);
    }

    let mut jobs_enqueued = Vec::new();
    let rebuild_key = format!("publish:{version}:rebuild");
    let rebuild_params = json!({ "skip_ingest": true });
    match tier {
        Some("C") => {
            let train_key = format!("publish:{version}:train");
            match pg::enqueue(&client, &org_row.id, "train", 0, Some(&train_key), None).await {
                Ok(j) => jobs_enqueued.push(j.id),
                Err(e) => return pg_err(e),
            }
            match pg::enqueue(
                &client,
                &org_row.id,
                "run_all_full",
                0,
                Some(&rebuild_key),
                Some(&rebuild_params),
            )
            .await
            {
                Ok(j) => jobs_enqueued.push(j.id),
                Err(e) => return pg_err(e),
            }
        }
        Some("A" | "B") => match pg::enqueue(
            &client,
            &org_row.id,
            "run_all_full",
            0,
            Some(&rebuild_key),
            Some(&rebuild_params),
        )
        .await
        {
            Ok(j) => jobs_enqueued.push(j.id),
            Err(e) => return pg_err(e),
        },
        _ => {}
    }

    let _ = pg::audit(
        &client,
        &principal.actor,
        Some(&org),
        "config.publish",
        &json!({ "version": version, "tier": tier }),
    )
    .await;
    (
        StatusCode::OK,
        Json(json!({
            "org": org,
            "version": version,
            "tier": tier,
            "changed_blocks": changed,
            "jobs_enqueued": jobs_enqueued,
            "published_by": principal.actor,
        })),
    )
        .into_response()
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
            Json(json!({ "detail": "kind required" })),
        )
            .into_response();
    }
    if let Some(dsn) = &state.dsn {
        let client = match connect_pg(dsn).await {
            Ok(c) => c,
            Err(r) => return *r,
        };
        let org_row = match org_or_404(&client, &org).await {
            Ok(r) => r,
            Err(r) => return *r,
        };
        return match pg::enqueue(&client, &org_row.id, &body.kind, body.priority, None, None).await
        {
            Ok(job) => {
                let _ = pg::audit(
                    &client,
                    &principal.actor,
                    Some(&org),
                    "job.submit",
                    &json!({ "job_id": job.id, "kind": body.kind }),
                )
                .await;
                (StatusCode::ACCEPTED, Json(json!(job))).into_response()
            }
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
    let now = chrono::Utc::now();
    let job = Job {
        id: job_id,
        org_id: org.to_string(),
        kind: kind.to_string(),
        params: serde_json::json!({}),
        state: "queued".to_string(),
        priority,
        idempotency_key: None,
        run_id: None,
        attempt: 0,
        max_attempts: 3,
        not_before: None,
        created_by: None,
        schedule_id: None,
        resource_class: None,
        trace_context: None,
        created_at: now,
        updated_at: now,
        started_at: None,
        finished_at: None,
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
        .find(|j| j.id == job.id)
        .expect("claimed job still present");
    slot.state = if outcome.is_ok() {
        "succeeded"
    } else {
        "failed"
    }
    .to_string();
    Some(slot.clone())
}

/// The REAL dispatcher step (`dispatch_next`'s Postgres twin, the control ->
/// data-plane bridge): claim the next runnable job from the SKIP-LOCKED queue
/// (`pg::claim`, which also enforces one-active-per-org), move it `running`, run
/// it via `runner`, then record `succeeded`/`failed`. Returns the job in its
/// final state, or `None` if nothing was claimable.
///
/// `runner` is injected so the queue mechanics stay backend-agnostic: the
/// production dispatcher process wires a runner that resolves the job's org to
/// its per-org DuckLake + published config + delivery/model inputs and calls
/// `er_cli::run_all` (proven end-to-end over a real DuckLake by the gated
/// `tests/pg_dispatch.rs`). The org -> inputs resolution (drop-file + model
/// registry lookup) is the remaining infrastructure-bound wiring.
///
/// # Errors
/// Propagates a queue (claim/mark) error. A `runner` failure is NOT an error —
/// it is recorded as the job's `failed` terminal state.
pub async fn dispatch_pg<F>(
    client: &tokio_postgres::Client,
    runner: F,
    webhook_http: Option<&reqwest::Client>,
) -> Result<Option<Job>, tokio_postgres::Error>
where
    F: FnOnce(&Job) -> Result<(), String>,
{
    let Some(job) = pg::claim(client).await? else {
        return Ok(None);
    };
    pg::mark(client, &job.id, "running").await?;
    let final_state = if runner(&job).is_ok() {
        "succeeded"
    } else {
        "failed"
    };
    pg::mark(client, &job.id, final_state).await?;
    let completed = Job {
        state: final_state.to_string(),
        ..job
    };
    // Best-effort webhook emit — a subscriber lookup / delivery failure never
    // fails the job (the outcome is already persisted).
    if let Some(http) = webhook_http {
        let _ = webhook::emit_job_event(client, http, &completed).await;
    }
    Ok(Some(completed))
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
        let org_row = match org_or_404(&client, &org).await {
            Ok(r) => r,
            Err(r) => return *r,
        };
        return match pg::list_jobs(&client, &org_row.id).await {
            Ok(jobs) => Json(json!({ "jobs": jobs })).into_response(),
            Err(e) => pg_err(e),
        };
    }
    let store = state.store.lock().unwrap();
    let jobs: Vec<&Job> = store
        .jobs
        .iter()
        .rev()
        .filter(|j| j.org_id == org)
        .collect();
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
        let org_row = match org_or_404(&client, &org).await {
            Ok(r) => r,
            Err(r) => return *r,
        };
        return match pg::get_job(&client, &org_row.id, &job_id).await {
            Ok(Some(job)) => (StatusCode::OK, Json(json!(job))).into_response(),
            Ok(None) => (
                StatusCode::NOT_FOUND,
                Json(json!({ "detail": "no such job" })),
            )
                .into_response(),
            Err(e) => pg_err(e),
        };
    }
    let store = state.store.lock().unwrap();
    match store
        .jobs
        .iter()
        .find(|j| j.org_id == org && j.id == job_id)
    {
        Some(job) => (StatusCode::OK, Json(json!(job))).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "detail": "no such job" })),
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
        let org_row = match org_or_404(&client, &org).await {
            Ok(r) => r,
            Err(r) => return *r,
        };
        return match pg::get_job(&client, &org_row.id, &job_id).await {
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
                Json(json!({ "detail": "no such job" })),
            )
                .into_response(),
            Err(e) => pg_err(e),
        };
    }
    let mut store = state.store.lock().unwrap();
    match store
        .jobs
        .iter_mut()
        .find(|j| j.org_id == org && j.id == job_id)
    {
        Some(job) => {
            job.state = "canceled".to_string();
            (StatusCode::OK, Json(json!(job.clone()))).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "detail": "no such job" })),
        )
            .into_response(),
    }
}

/// Steward+: requeue a failed/canceled job (`POST /v1/orgs/{org}/jobs/{job_id}/
/// resume`; the Python `{job_id}:resume` action). 409 if the job isn't in a
/// resumable state. Postgres-only (the in-memory store has no resume path).
async fn resume_job(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, job_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "resume requires Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    match pg::resume_job(&client, &org_row.id, &job_id).await {
        Ok(true) => {
            let _ = pg::audit(
                &client,
                &principal.actor,
                Some(&org),
                "job.resume",
                &json!({ "job_id": job_id }),
            )
            .await;
            (
                StatusCode::OK,
                Json(json!({ "job_id": job_id, "state": "queued" })),
            )
                .into_response()
        }
        Ok(false) => detail_err(
            StatusCode::CONFLICT,
            "job is not resumable (not failed or canceled)",
        ),
        Err(e) => pg_err(e),
    }
}

/// The job-state vocabulary, for validating the `?state` filter.
const JOB_STATES: [&str; 7] = [
    "queued",
    "dispatching",
    "running",
    "canceling",
    "succeeded",
    "failed",
    "canceled",
];

/// Operator: the cross-org queue view (`GET /v1/jobs`), optional `org`/`state`
/// filters, newest first. A bare array.
async fn jobs_global(
    principal: Principal,
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = operator_only(&principal) {
        return e.into_response();
    }
    if let Some(s) = params.get("state")
        && !JOB_STATES.contains(&s.as_str())
    {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("unknown state '{s}'"),
        );
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "job console requires Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let limit = read::parse_limit(params.get("limit").map(String::as_str));
    // The ?org= filter is the org NAME (the console's handle); resolve it to the
    // id child rows carry. An unknown name filters to nothing, like before.
    let org_filter = match params.get("org") {
        Some(name) => match pg::get_org(&client, name).await {
            Ok(Some(o)) => Some(o.id),
            Ok(None) => Some(String::from("\u{0}no-such-org")),
            Err(e) => return pg_err(e),
        },
        None => None,
    };
    match pg::list_jobs_all(
        &client,
        org_filter.as_deref(),
        params.get("state").map(String::as_str),
        limit,
    )
    .await
    {
        Ok(jobs) => Json(json!(jobs)).into_response(),
        Err(e) => pg_err(e),
    }
}

/// Read the audit ledger from a request's query params, newest first.
async fn audit_feed(
    client: &tokio_postgres::Client,
    org: Option<&str>,
    params: &HashMap<String, String>,
) -> Result<Vec<Value>, Box<axum::response::Response>> {
    let action = params.get("action").map(String::as_str);
    let before_id = params.get("before_id").and_then(|s| s.parse::<i64>().ok());
    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(100)
        .clamp(1, 500);
    pg::audit_rows(client, org, action, before_id, limit)
        .await
        .map_err(|e| Box::new(pg_err(e)))
}

/// Operator: the cross-tenant audit feed (`GET /v1/audit`), optional
/// `org`/`action`/`before_id` filters.
async fn audit_global(
    principal: Principal,
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = operator_only(&principal) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "audit requires Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    match audit_feed(&client, params.get("org").map(String::as_str), &params).await {
        Ok(rows) => Json(json!(rows)).into_response(),
        Err(r) => *r,
    }
}

/// Admin+: an org's audit feed (`GET /v1/orgs/{org}/audit`).
async fn audit_index(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Admin) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "audit requires Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    match pg::org_exists(&client, &org).await {
        Ok(true) => {}
        Ok(false) => return detail_err(StatusCode::NOT_FOUND, format!("no org '{org}'")),
        Err(e) => return pg_err(e),
    }
    match audit_feed(&client, Some(&org), &params).await {
        Ok(rows) => Json(json!(rows)).into_response(),
        Err(r) => *r,
    }
}

#[derive(Deserialize)]
struct ResourcesIn {
    #[serde(default)]
    duckdb_threads: Option<i64>,
    #[serde(default)]
    duckdb_memory_limit: Option<String>,
}

/// Operator: set an org's DuckDB parallelism knobs
/// (`PATCH /v1/orgs/{org}/resources`), merged into its `env`. 422 if neither
/// field is given; returns the resulting `resources` view.
async fn update_resources(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<ResourcesIn>,
) -> impl IntoResponse {
    if let Err(e) = operator_only(&principal) {
        return e.into_response();
    }
    let mut updates = serde_json::Map::new();
    if let Some(threads) = body.duckdb_threads {
        updates.insert("ER_DUCKDB_THREADS".to_string(), json!(threads.to_string()));
    }
    if let Some(mem) = body.duckdb_memory_limit.filter(|s| !s.is_empty()) {
        updates.insert("ER_DUCKDB_MEMORY_LIMIT".to_string(), json!(mem));
    }
    if updates.is_empty() {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "provide duckdb_threads and/or duckdb_memory_limit",
        );
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "org registry requires Postgres",
        );
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    match pg::org_exists(&client, &org).await {
        Ok(true) => {}
        Ok(false) => return detail_err(StatusCode::NOT_FOUND, format!("no org '{org}'")),
        Err(e) => return pg_err(e),
    }
    if let Err(e) = pg::merge_org_env(&client, &org, &Value::Object(updates)).await {
        return pg_err(e);
    }
    match pg::get_org(&client, &org).await {
        Ok(Some(o)) => Json(resources_of(&o.env)).into_response(),
        Ok(None) => detail_err(StatusCode::NOT_FOUND, format!("no org '{org}'")),
        Err(e) => pg_err(e),
    }
}

/// Open the requesting org's lake read-only, resolving the org row first.
/// Returns the attached session, or a ready error response (404 no org, 501 no
/// Postgres backend, 503 lake unavailable).
async fn org_lake(
    state: &AppState,
    org: &str,
) -> Result<dedupe_core::Session, Box<axum::response::Response>> {
    let Some(dsn) = &state.dsn else {
        return Err(Box::new(detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "read API requires Postgres",
        )));
    };
    let client = connect_pg(dsn).await.map_err(|r| Box::new(*r))?;
    let env = match pg::get_org(&client, org).await {
        Ok(Some(o)) => o.env,
        Ok(None) => {
            return Err(Box::new(detail_err(
                StatusCode::NOT_FOUND,
                format!("no org '{org}'"),
            )));
        }
        Err(e) => {
            return Err(Box::new(detail_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db: {e}"),
            )));
        }
    };
    read::open_org_lake(&env).map_err(|e| {
        Box::new(detail_err(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("lake unavailable: {e}"),
        ))
    })
}

/// Like [`org_lake`], but also resolves the org's auto-merge threshold from its
/// config (for the run ledger's per-run auto-merge count, PR 1.5b). One
/// `get_org`. The threshold is `None` when the org has no readable config.
async fn org_lake_auto_merge(
    state: &AppState,
    org: &str,
) -> Result<(dedupe_core::Session, Option<f64>), Box<axum::response::Response>> {
    let Some(dsn) = &state.dsn else {
        return Err(Box::new(detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "read API requires Postgres",
        )));
    };
    let client = connect_pg(dsn).await.map_err(|r| Box::new(*r))?;
    let env = match pg::get_org(&client, org).await {
        Ok(Some(o)) => o.env,
        Ok(None) => {
            return Err(Box::new(detail_err(
                StatusCode::NOT_FOUND,
                format!("no org '{org}'"),
            )));
        }
        Err(e) => {
            return Err(Box::new(detail_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db: {e}"),
            )));
        }
    };
    let auto_merge = auto_merge_from_env(&env);
    let session = read::open_org_lake(&env).map_err(|e| {
        Box::new(detail_err(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("lake unavailable: {e}"),
        ))
    })?;
    Ok((session, auto_merge))
}

/// The org's entity type, from its `ER_CONFIG_PATH` yaml. Person when the key is
/// absent, the file is unreadable, or the config omits `entity_type` — so person
/// orgs read exactly as before.
fn entity_type_from_env(env: &Value) -> er_config::EntityType {
    env.get("ER_CONFIG_PATH")
        .and_then(Value::as_str)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| load_config_str(&t).ok())
        .map(|c| c.entity_type)
        .unwrap_or_default()
}

/// The org's auto-merge probability threshold, from its `ER_CONFIG_PATH` yaml.
/// `None` when the key is absent or the file/config is unreadable — the run
/// ledger then omits the per-run auto-merge count rather than report it against a
/// guessed threshold (Thresholds has no Default to fall back to).
fn auto_merge_from_env(env: &Value) -> Option<f64> {
    env.get("ER_CONFIG_PATH")
        .and_then(Value::as_str)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| load_config_str(&t).ok())
        .map(|c| c.thresholds.auto_merge)
}

/// Like [`org_lake`], but also resolves the org's entity type (one `get_org`).
/// The golden/entity reads dispatch on it so an Account org reads its
/// `acct_golden_*` projection; a person org is unchanged.
async fn org_lake_golden(
    state: &AppState,
    org: &str,
) -> Result<(dedupe_core::Session, er_config::EntityType), Box<axum::response::Response>> {
    let Some(dsn) = &state.dsn else {
        return Err(Box::new(detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "read API requires Postgres",
        )));
    };
    let client = connect_pg(dsn).await.map_err(|r| Box::new(*r))?;
    let env = match pg::get_org(&client, org).await {
        Ok(Some(o)) => o.env,
        Ok(None) => {
            return Err(Box::new(detail_err(
                StatusCode::NOT_FOUND,
                format!("no org '{org}'"),
            )));
        }
        Err(e) => {
            return Err(Box::new(detail_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db: {e}"),
            )));
        }
    };
    let entity_type = entity_type_from_env(&env);
    let session = read::open_org_lake(&env).map_err(|e| {
        Box::new(detail_err(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("lake unavailable: {e}"),
        ))
    })?;
    Ok((session, entity_type))
}

/// Viewer+: a keyset page of the org's golden (resolved) records
/// (`GET /v1/orgs/{org}/golden-records`). Query: `q` (contains-search), `cursor`
/// (opaque), `limit` (<=200). Reads the org's DuckLake at a pinned snapshot.
async fn golden_records_list(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let (session, entity_type) = match org_lake_golden(&state, &org).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let (snapshot, after) =
        match read::page_start(&session, params.get("cursor").map(String::as_str)) {
            Ok(x) => x,
            Err(read::PageStartError::BadCursor) => {
                return detail_err(StatusCode::UNPROCESSABLE_ENTITY, "invalid cursor");
            }
            Err(read::PageStartError::Lake(e)) => {
                return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}"));
            }
        };
    let limit = read::parse_limit(params.get("limit").map(String::as_str));
    let page = match read::golden_arrow_page(
        &session,
        entity_type,
        snapshot,
        params.get("q").map(String::as_str),
        after.as_deref(),
        limit,
    ) {
        Ok(p) => p,
        Err(e) => return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    };

    // Serialise the page as an Arrow IPC stream (schema message then batches); the
    // snapshot and the next cursor travel in response headers.
    let mut buf: Vec<u8> = Vec::new();
    match arrow::ipc::writer::StreamWriter::try_new(&mut buf, &page.schema) {
        Ok(mut writer) => {
            for batch in &page.batches {
                if let Err(e) = writer.write(batch) {
                    return detail_err(StatusCode::INTERNAL_SERVER_ERROR, format!("arrow: {e}"));
                }
            }
            if let Err(e) = writer.finish() {
                return detail_err(StatusCode::INTERNAL_SERVER_ERROR, format!("arrow: {e}"));
            }
        }
        Err(e) => return detail_err(StatusCode::INTERNAL_SERVER_ERROR, format!("arrow: {e}")),
    }

    let mut response = axum::response::Response::new(axum::body::Body::from(buf));
    let headers = response.headers_mut();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/vnd.apache.arrow.stream"),
    );
    if let Ok(v) = axum::http::HeaderValue::from_str(&snapshot.to_string()) {
        headers.insert("x-er-snapshot", v);
    }
    if let Some(cursor) = &page.next_cursor {
        if let Ok(v) = axum::http::HeaderValue::from_str(cursor) {
            headers.insert("x-er-next-cursor", v);
        }
    }
    response
}

/// Viewer+: one resolved entity with members/lineage/events
/// (`GET /v1/orgs/{org}/golden-records/{entity_id}`), 404 if absent.
async fn golden_record_detail(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, entity_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let (session, entity_type) = match org_lake_golden(&state, &org).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let snapshot = match read::latest_snapshot(&session) {
        Ok(s) => s,
        Err(e) => return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}")),
    };
    match read::entity_detail(&session, entity_type, snapshot, &entity_id) {
        Ok(Some(detail)) => Json(detail).into_response(),
        Ok(None) => detail_err(StatusCode::NOT_FOUND, format!("no entity '{entity_id}'")),
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
}

/// Viewer+: dashboard counters over the org's lake (`GET /v1/orgs/{org}/metrics`).
async fn org_metrics(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match org_lake(&state, &org).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let snapshot = match read::latest_snapshot(&session) {
        Ok(s) => s,
        Err(e) => return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}")),
    };
    match read::metrics(&session, snapshot) {
        Ok(m) => Json(m).into_response(),
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
}

/// Viewer+: data-quality scores over the org's lake
/// (`GET /v1/orgs/{org}/data-quality?stale_days=N`). Read-only analytics;
/// `stale_days` (default 365) is a presentation parameter, never config.
async fn org_data_quality(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let stale_days = params
        .get("stale_days")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(365)
        .clamp(1, 36_500);
    let session = match org_lake(&state, &org).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let snapshot = match read::latest_snapshot(&session) {
        Ok(s) => s,
        Err(e) => return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}")),
    };
    match read::data_quality(&session, snapshot, stale_days) {
        Ok(m) => Json(m).into_response(),
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
}

/// Viewer+: duplicate groups (entities with >=2 members)
/// (`GET /v1/orgs/{org}/duplicates`). Keyset page; query `cursor`, `limit`.
async fn duplicates_list(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let (session, entity_type) = match org_lake_golden(&state, &org).await {
        Ok(sv) => sv,
        Err(r) => return *r,
    };
    let (snapshot, after) =
        match read::page_start(&session, params.get("cursor").map(String::as_str)) {
            Ok(x) => x,
            Err(read::PageStartError::BadCursor) => {
                return detail_err(StatusCode::UNPROCESSABLE_ENTITY, "invalid cursor");
            }
            Err(read::PageStartError::Lake(e)) => {
                return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}"));
            }
        };
    let limit = read::parse_limit(params.get("limit").map(String::as_str));
    match read::duplicates(&session, entity_type, snapshot, after.as_deref(), limit) {
        Ok((items, next_cursor)) => Json(json!({
            "items": items,
            "snapshot": snapshot,
            "next_cursor": next_cursor,
        }))
        .into_response(),
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
}

/// Viewer+: active pairwise match scores (`GET /v1/orgs/{org}/match-scores`).
/// Keyset page; query `cursor`, `limit`, `min`/`max` (probability band).
async fn match_scores_list(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match org_lake(&state, &org).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let (snapshot, after) =
        match read::page_start(&session, params.get("cursor").map(String::as_str)) {
            Ok(x) => x,
            Err(read::PageStartError::BadCursor) => {
                return detail_err(StatusCode::UNPROCESSABLE_ENTITY, "invalid cursor");
            }
            Err(read::PageStartError::Lake(e)) => {
                return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}"));
            }
        };
    let limit = read::parse_limit(params.get("limit").map(String::as_str));
    let min = read::parse_f64(params.get("min").map(String::as_str));
    let max = read::parse_f64(params.get("max").map(String::as_str));
    match read::match_scores_list(&session, snapshot, after.as_deref(), min, max, limit) {
        Ok((items, next_cursor)) => Json(json!({
            "items": items,
            "snapshot": snapshot,
            "next_cursor": next_cursor,
        }))
        .into_response(),
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
}

/// Steward+: a READ-ONLY score-only match preview (frontend §7.16,
/// `GET /v1/orgs/{org}/match-preview`). Runs the committed model over the current
/// corpus and returns the pairs it WOULD score, bucketed by the config
/// thresholds, with a sample carrying the evidence waterfall. Writes nothing —
/// no `match_scores` / `review_queue` / golden, no determinism-contract artifact.
/// `?limit=` caps the sample. The matcher is heavy + holds a !Send session, so it
/// runs on a blocking thread.
async fn match_preview(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "match preview requires Postgres",
        );
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let env = match pg::get_org(&client, &org).await {
        Ok(Some(o)) if o.state == "active" => o.env,
        Ok(Some(o)) => {
            return detail_err(
                StatusCode::CONFLICT,
                format!("org '{org}' is {}; preview needs an active org", o.state),
            );
        }
        Ok(None) => return detail_err(StatusCode::NOT_FOUND, format!("no org '{org}'")),
        Err(e) => return pg_err(e),
    };
    let limit = read::parse_limit(params.get("limit").map(String::as_str))
        .try_into()
        .unwrap_or(50usize);

    match tokio::task::spawn_blocking(move || match_preview_blocking(&env, limit)).await {
        Ok(Ok(body)) => Json(body).into_response(),
        Ok(Err(msg)) => detail_err(StatusCode::CONFLICT, msg),
        Err(e) => detail_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("preview task panicked: {e}"),
        ),
    }
}

/// The blocking half of [`match_preview`]: open the org lake + frozen model from
/// its env, read the thresholds from its config, run the read-only
/// `er_cli::preview_match`, and render the JSON body. Any missing input (no
/// trained model yet) returns a human string the handler maps to 409.
fn match_preview_blocking(env: &Value, limit: usize) -> Result<Value, String> {
    let s = |k: &str| env.get(k).and_then(Value::as_str).filter(|v| !v.is_empty());
    let catalog = s("ER_LAKE_CATALOG").ok_or("org env missing ER_LAKE_CATALOG")?;
    let data_path = s("ER_LAKE_DATA_PATH").ok_or("org env missing ER_LAKE_DATA_PATH")?;
    let model_path =
        s("ER_MODEL_PATH").ok_or("no trained model yet (ER_MODEL_PATH); run a full run first")?;
    let tf_path =
        s("ER_TF_PATH").ok_or("no trained model yet (ER_TF_PATH); run a full run first")?;
    let config_path = s("ER_CONFIG_PATH").ok_or("org env missing ER_CONFIG_PATH")?;

    let settings_json =
        std::fs::read_to_string(model_path).map_err(|e| format!("read model: {e}"))?;
    let config_text =
        std::fs::read_to_string(config_path).map_err(|e| format!("read config: {e}"))?;
    let config = er_config::load_config_str(&config_text)
        .map_err(|e| format!("config: {}", errfmt::chain(&e)))?;
    let session = er_cli::open_lake(catalog, data_path).map_err(|e| format!("open lake: {e:#}"))?;
    let model = er_cli::MatchModel {
        settings_json: &settings_json,
        tf_csv_path: std::path::Path::new(tf_path),
        model_version: s("ER_MODEL_VERSION").unwrap_or("v0001"),
        tf_snapshot_id: s("ER_TF_SNAPSHOT_ID").unwrap_or("tf"),
    };
    let preview = er_cli::preview_match(
        &session,
        &model,
        config.thresholds.review_low,
        config.thresholds.auto_merge,
        limit,
    )
    .map_err(|e| format!("preview: {e:#}"))?;

    let sample: Vec<Value> = preview
        .sample
        .iter()
        .map(|p| {
            json!({
                "rec_a_key": p.rec_a_key,
                "rec_b_key": p.rec_b_key,
                "match_probability": p.match_probability,
                "evidence": serde_json::from_str::<Value>(&p.evidence).unwrap_or(Value::Null),
            })
        })
        .collect();
    Ok(json!({
        "total": preview.total,
        "would_auto_merge": preview.would_auto_merge,
        "would_review": preview.would_review,
        "sample": sample,
    }))
}

/// Viewer+: standardized source records by `record_key`
/// (`GET /v1/orgs/{org}/records?key=…`, `key` repeats, capped at 50). Not
/// paginated — a bounded lookup. Returns `{ items, snapshot }`.
async fn records_by_key(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match org_lake(&state, &org).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let snapshot = match read::latest_snapshot(&session) {
        Ok(s) => s,
        Err(e) => return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}")),
    };
    let keys = read::keys_from_query(query.as_deref());
    match read::records_by_key(&session, snapshot, &keys) {
        Ok(items) => Json(json!({ "items": items, "snapshot": snapshot })).into_response(),
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
}

/// Resolve `(snapshot, after)` from the `cursor` query param, or a ready error
/// response (422 bad cursor, 503 lake). Shared by the keyset read handlers.
fn page_or_err(
    session: &dedupe_core::Session,
    cursor: Option<&str>,
) -> Result<(i64, Option<String>), Box<axum::response::Response>> {
    match read::page_start(session, cursor) {
        Ok(x) => Ok(x),
        Err(read::PageStartError::BadCursor) => Err(Box::new(detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid cursor",
        ))),
        Err(read::PageStartError::Lake(e)) => Err(Box::new(detail_err(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("lake: {e}"),
        ))),
    }
}

/// `{ items, next_cursor }` for a keyset reader result (the reviews/assertions/
/// imports envelope — no `snapshot` key, matching the Python readapi).
fn page_envelope(r: Result<(Vec<Value>, Option<String>), String>) -> axum::response::Response {
    match r {
        Ok((items, next_cursor)) => {
            Json(json!({ "items": items, "next_cursor": next_cursor })).into_response()
        }
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
}

/// Viewer+: the open-review queue (`GET /v1/orgs/{org}/reviews`).
async fn reviews_list(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match org_lake(&state, &org).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let (snapshot, after) = match page_or_err(&session, params.get("cursor").map(String::as_str)) {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let limit = read::parse_limit(params.get("limit").map(String::as_str));
    page_envelope(read::reviews_list(
        &session,
        snapshot,
        after.as_deref(),
        limit,
    ))
}

/// Viewer+: one review by id (`GET /v1/orgs/{org}/reviews/{review_id}`), 404 if absent.
async fn review_detail(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, review_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match org_lake(&state, &org).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let snapshot = match read::latest_snapshot(&session) {
        Ok(s) => s,
        Err(e) => return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}")),
    };
    match read::review_get(&session, snapshot, &review_id) {
        Ok(Some(r)) => Json(r).into_response(),
        Ok(None) => detail_err(StatusCode::NOT_FOUND, format!("no review '{review_id}'")),
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
}

/// Viewer+: the assertion ledger (`GET /v1/orgs/{org}/assertions`), active-only
/// unless `?include_retracted=true`.
async fn assertions_list(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match org_lake(&state, &org).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let (snapshot, after) = match page_or_err(&session, params.get("cursor").map(String::as_str)) {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let include_retracted = matches!(
        params.get("include_retracted").map(String::as_str),
        Some("true" | "1")
    );
    let limit = read::parse_limit(params.get("limit").map(String::as_str));
    page_envelope(read::assertions_list(
        &session,
        snapshot,
        include_retracted,
        after.as_deref(),
        limit,
    ))
}

/// Viewer+: ingest-batch receipts (`GET /v1/orgs/{org}/imports`), optional
/// `?source` filter.
async fn imports_list(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match org_lake(&state, &org).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let (snapshot, after) = match page_or_err(&session, params.get("cursor").map(String::as_str)) {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let limit = read::parse_limit(params.get("limit").map(String::as_str));
    page_envelope(read::imports_list(
        &session,
        snapshot,
        params.get("source").map(String::as_str),
        after.as_deref(),
        limit,
    ))
}

/// Viewer+: the 20 most recent runs (`GET /v1/orgs/{org}/runs`), a bare array.
async fn runs_list(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let (session, auto_merge) = match org_lake_auto_merge(&state, &org).await {
        Ok(pair) => pair,
        Err(r) => return *r,
    };
    let snapshot = match read::latest_snapshot(&session) {
        Ok(s) => s,
        Err(e) => return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}")),
    };
    match read::runs_list(&session, snapshot, auto_merge) {
        Ok(items) => Json(json!(items)).into_response(),
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
}

/// Load an org's `source_system -> priority_rank` map from its active published
/// config (needed only by the `source_priority` master-election policy).
async fn org_source_ranks(
    state: &AppState,
    org: &str,
) -> Result<std::collections::BTreeMap<String, i64>, Box<axum::response::Response>> {
    let Some(dsn) = &state.dsn else {
        return Err(Box::new(detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "read API requires Postgres",
        )));
    };
    let client = connect_pg(dsn).await?;
    let active = match pg::org_publish_context(&client, org).await {
        Ok(Some((_, _, Some(v)))) => v,
        Ok(_) => {
            return Err(Box::new(detail_err(
                StatusCode::NOT_FOUND,
                "no published config",
            )));
        }
        Err(e) => {
            return Err(Box::new(detail_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db: {e}"),
            )));
        }
    };
    let yaml = match pg::get_config_version(&client, org, active).await {
        Ok(Some(d)) => d.yaml,
        Ok(None) => {
            return Err(Box::new(detail_err(
                StatusCode::NOT_FOUND,
                "no published config",
            )));
        }
        Err(e) => {
            return Err(Box::new(detail_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db: {e}"),
            )));
        }
    };
    let config =
        load_config_str(&yaml).map_err(|e| Box::new(config_invalid(&e).into_response()))?;
    Ok(config
        .sources
        .iter()
        .map(|(name, spec)| (name.clone(), spec.priority_rank))
        .collect())
}

/// Viewer+: merge plans for entities touched by a merge/member-add
/// (`GET /v1/orgs/{org}/merge-plans`). Query: `policy` (master-election, default
/// `most_attributes`), `since` (ISO8601), `limit`, `format` (json|csv).
async fn merge_plans(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let policy = params
        .get("policy")
        .map_or(er_golden::DEFAULT_MASTER_ELECTION_POLICY, String::as_str);
    if !er_golden::MASTER_ELECTION_POLICIES.contains(&policy) {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("unknown master-election policy '{policy}'"),
        );
    }
    // source_priority is the only policy that needs the config's source ranks.
    let source_ranks = if policy == "source_priority" {
        match org_source_ranks(&state, &org).await {
            Ok(r) => Some(r),
            Err(r) => return *r,
        }
    } else {
        None
    };
    let session = match org_lake(&state, &org).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let limit = read::parse_limit(params.get("limit").map(String::as_str));
    let items = match read::merge_plans(
        &session,
        policy,
        params.get("since").map(String::as_str),
        source_ranks.as_ref(),
        limit,
    ) {
        Ok(items) => items,
        Err(e) => return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    };
    if params.get("format").map(String::as_str) == Some("csv") {
        let csv = read::merge_plans_csv(&items);
        (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "text/csv; charset=utf-8")],
            csv,
        )
            .into_response()
    } else {
        Json(json!({ "items": items })).into_response()
    }
}

/// Job kinds that may be scheduled (`erserver.policy.SCHEDULABLE_KINDS`).
const SCHEDULABLE_KINDS: [&str; 5] = [
    "run_all_full",
    "run_all_incremental",
    "correct",
    "train",
    "lake_maintain",
];

/// A lightweight 5-field cron check (looser than croniter: field count + the
/// cron character class; it does not validate ranges/step bounds).
fn valid_cron(s: &str) -> bool {
    let fields: Vec<&str> = s.split_whitespace().collect();
    fields.len() == 5
        && fields.iter().all(|f| {
            !f.is_empty()
                && f.bytes()
                    .all(|b| b.is_ascii_digit() || b"*/,-".contains(&b))
        })
}

#[derive(Deserialize)]
struct ScheduleIn {
    kind: String,
    cron: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Deserialize)]
struct ScheduleUpdateIn {
    enabled: bool,
}

/// Admin+: create an API-sourced schedule (`POST /v1/orgs/{org}/schedules`).
async fn create_schedule(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<ScheduleIn>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Admin) {
        return e.into_response();
    }
    if !SCHEDULABLE_KINDS.contains(&body.kind.as_str()) {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("kind '{}' is not schedulable", body.kind),
        );
    }
    if !valid_cron(&body.cron) {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("invalid cron expression: '{}'", body.cron),
        );
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "schedules require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let params = body.params.unwrap_or_else(|| json!({}));
    // History images record the acting principal (er.actor).
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::create_schedule(&client, &org_row.id, &body.kind, &body.cron, &params).await {
        Ok(schedule_id) => (
            StatusCode::CREATED,
            Json(json!({ "id": schedule_id, "kind": body.kind, "cron": body.cron })),
        )
            .into_response(),
        Err(e) => pg_err(e),
    }
}

/// Viewer+: list an org's schedules (`GET /v1/orgs/{org}/schedules`).
async fn schedules_index(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "schedules require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    match pg::list_schedules(&client, &org_row.id).await {
        Ok(rows) => {
            let out: Vec<Value> = rows
                .iter()
                .map(|s| {
                    json!({
                        "id": s.id,
                        "kind": s.kind,
                        "cron": s.cron,
                        "enabled": s.enabled,
                        "source": s.source,
                        "last_enqueued_at": s.last_enqueued_at,
                    })
                })
                .collect();
            Json(json!(out)).into_response()
        }
        Err(e) => pg_err(e),
    }
}

/// Admin+: enable/disable a schedule (`PATCH /v1/orgs/{org}/schedules/{id}`).
/// Config/system-owned rows refuse (409).
async fn update_schedule(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, schedule_id)): Path<(String, String)>,
    Json(body): Json<ScheduleUpdateIn>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Admin) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "schedules require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    // History images record the acting principal (er.actor).
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::set_schedule_enabled(&client, &org_row.id, &schedule_id, body.enabled).await {
        Ok(true) => Json(json!({ "id": schedule_id, "enabled": body.enabled })).into_response(),
        Ok(false) => detail_err(
            StatusCode::CONFLICT,
            "schedule is unknown, belongs to another org, or is config-owned",
        ),
        Err(e) => pg_err(e),
    }
}

/// Admin+: delete a schedule (`DELETE /v1/orgs/{org}/schedules/{id}`).
async fn delete_schedule(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, schedule_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Admin) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "schedules require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    // History images record the acting principal (er.actor).
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::delete_schedule(&client, &org_row.id, &schedule_id).await {
        Ok(deleted) => Json(json!({ "deleted": deleted })).into_response(),
        Err(e) => pg_err(e),
    }
}

#[derive(Deserialize)]
struct WebhookIn {
    url: String,
    #[serde(default)]
    events: Vec<String>,
    #[serde(default)]
    secret: Option<String>,
}

/// A fresh HMAC signing secret: 24 random bytes, hex-encoded (same shape as an
/// API-key secret).
fn gen_webhook_secret() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 24];
    rand::rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

/// Admin+: register an outbound webhook (`POST /v1/orgs/{org}/webhooks`). The
/// HMAC signing `secret` is caller-supplied or generated, and returned ONCE
/// (never listed thereafter, like an API key).
///
/// Delivery — signing each event payload with the secret and POSTing it to
/// `url` — is the infra-bound part (a background sender, not run in this
/// sandbox); this endpoint manages the registration.
async fn create_webhook(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<WebhookIn>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Admin) {
        return e.into_response();
    }
    if body.url.trim().is_empty() {
        return detail_err(StatusCode::UNPROCESSABLE_ENTITY, "url is required");
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "webhooks require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let secret = body
        .secret
        .filter(|s| !s.is_empty())
        .unwrap_or_else(gen_webhook_secret);
    let events = json!(body.events);
    // History images record the acting principal (er.actor).
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::create_webhook(&client, &org_row.id, &body.url, &events, &secret).await {
        Ok(webhook_id) => {
            let _ = pg::audit(
                &client,
                &principal.actor,
                Some(&org),
                "webhook.create",
                &json!({ "webhook_id": webhook_id, "url": body.url }),
            )
            .await;
            (
                StatusCode::CREATED,
                Json(json!({
                    "id": webhook_id,
                    "url": body.url,
                    "events": events,
                    "secret": secret,
                    "active": true,
                })),
            )
                .into_response()
        }
        Err(e) => pg_err(e),
    }
}

/// Viewer+: list an org's active webhooks (`GET /v1/orgs/{org}/webhooks`); the
/// signing secret is never listed.
async fn webhooks_index(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "webhooks require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    match pg::list_webhooks(&client, &org_row.id).await {
        Ok(rows) => {
            let out: Vec<Value> = rows
                .iter()
                .map(|w| {
                    json!({
                        "id": w.id,
                        "url": w.url,
                        "events": w.events,
                        "active": w.active,
                    })
                })
                .collect();
            Json(json!(out)).into_response()
        }
        Err(e) => pg_err(e),
    }
}

/// Admin+: delete a webhook (`DELETE /v1/orgs/{org}/webhooks/{webhook_id}`).
async fn delete_webhook(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, webhook_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Admin) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "webhooks require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    // History images record the acting principal (er.actor).
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::delete_webhook(&client, &org_row.id, &webhook_id).await {
        Ok(deleted) => {
            if deleted {
                let _ = pg::audit(
                    &client,
                    &principal.actor,
                    Some(&org),
                    "webhook.delete",
                    &json!({ "webhook_id": webhook_id }),
                )
                .await;
            }
            Json(json!({ "deleted": deleted })).into_response()
        }
        Err(e) => pg_err(e),
    }
}

/// Stage a steward action (the writer-lock-held path of `steward.py`
/// `try_apply_or_stage`): the dispatcher drains it when the org goes quiet. The
/// immediate-apply path needs the per-tenant writer lock on the postgres-backed
/// ducklake catalog (infra-bound), so this always stages. Returns the staged
/// `action_id` or a ready error response (404 no org, 409 not active).
async fn steward_stage(
    client: &tokio_postgres::Client,
    actor: &str,
    org: &str,
    action: &Value,
) -> Result<String, Box<axum::response::Response>> {
    let detail = match pg::get_org(client, org).await {
        Ok(Some(o)) => o,
        Ok(None) => {
            return Err(Box::new(detail_err(
                StatusCode::NOT_FOUND,
                format!("no org '{org}'"),
            )));
        }
        Err(e) => return Err(Box::new(pg_err(e))),
    };
    if detail.state != "active" {
        return Err(Box::new(detail_err(
            StatusCode::CONFLICT,
            format!(
                "org '{org}' is {}; steward actions are accepted only when active",
                detail.state
            ),
        )));
    }
    let action_type = action
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let action_id = pg::stage_action(client, &detail.id, action, actor)
        .await
        .map_err(|e| Box::new(pg_err(e)))?;
    let _ = pg::audit(
        client,
        actor,
        Some(org),
        &format!("steward.{action_type}"),
        &json!({ "action_id": action_id, "status": "staged" }),
    )
    .await;
    Ok(action_id)
}

/// The staged-action response envelope (`_steward_action`): always `staged`
/// here, so `pending_until_next_reconcile` is false (immediate apply is the
/// infra-bound writer-lock path).
fn staged_response(status: StatusCode, action_id: &str) -> axum::response::Response {
    (
        status,
        Json(json!({
            "status": "staged",
            "action_id": action_id,
            "pending_until_next_reconcile": false,
        })),
    )
        .into_response()
}

#[derive(Deserialize)]
struct AssertionIn {
    kind: String,
    a: String,
    b: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    apply_now: bool,
}

/// Steward+: stage an always/never assertion on a pair
/// (`POST /v1/orgs/{org}/assertions`). With `apply_now`, also enqueue an
/// incremental rebuild that drains + applies it.
async fn add_assertion(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<AssertionIn>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    if body.kind != "always" && body.kind != "never" {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "kind must be always | never",
        );
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "steward actions require Postgres",
        );
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let action = json!({
        "type": "add_assertion",
        "kind": body.kind,
        "a": body.a,
        "b": body.b,
        "created_by": principal.actor,
        "note": body.note,
    });
    let action_id = match steward_stage(&client, &principal.actor, &org, &action).await {
        Ok(id) => id,
        Err(r) => return *r,
    };
    if !body.apply_now {
        return staged_response(StatusCode::CREATED, &action_id);
    }
    // apply_now: enqueue the steward drain — apply the staged actions into the
    // lake, then correct (reconcile/golden/reviews over the existing scores).
    let idem = format!("assert-apply:{}", er_ids::new_v7());
    let params = json!({ "skip_ingest": true });
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    match pg::enqueue(
        &client,
        &org_row.id,
        "apply_corrections",
        0,
        Some(&idem),
        Some(&params),
    )
    .await
    {
        Ok(job) => (
            StatusCode::CREATED,
            Json(json!({
                "status": "staged",
                "action_id": action_id,
                "pending_until_next_reconcile": false,
                "apply_job": job.id,
            })),
        )
            .into_response(),
        Err(e) => pg_err(e),
    }
}

#[derive(Deserialize)]
struct SuppressIn {
    record_key: String,
    #[serde(default)]
    apply_now: bool,
}

/// Steward+: stage a record overlay (`overlay` = `"suppress"` or `"protect"`), or
/// retract one. `retract=false` stages `{overlay}_record`; `retract=true` stages
/// `retract_record_overlay` (carrying `kind`). A `suppress` overlay drops the
/// record from reconcile clustering; a `protect` overlay forces it master on the
/// read path. Unlike a source tombstone, an overlay survives re-ingest (every run
/// re-reads it). With `apply_now` it enqueues the steward drain + correct.
async fn record_overlay_action(
    principal: Principal,
    state: AppState,
    org: String,
    body: SuppressIn,
    overlay: &'static str,
    retract: bool,
) -> axum::response::Response {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    if body.record_key.trim().is_empty() {
        return detail_err(StatusCode::UNPROCESSABLE_ENTITY, "record_key is required");
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "steward actions require Postgres",
        );
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let action = if retract {
        json!({
            "type": "retract_record_overlay",
            "record_key": body.record_key,
            "kind": overlay,
            "created_by": principal.actor,
        })
    } else {
        json!({
            "type": format!("{overlay}_record"),
            "record_key": body.record_key,
            "created_by": principal.actor,
        })
    };
    let action_id = match steward_stage(&client, &principal.actor, &org, &action).await {
        Ok(id) => id,
        Err(r) => return *r,
    };
    if !body.apply_now {
        return staged_response(StatusCode::CREATED, &action_id);
    }
    let idem = format!("{overlay}-apply:{}", er_ids::new_v7());
    let params = json!({ "skip_ingest": true });
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    match pg::enqueue(
        &client,
        &org_row.id,
        "apply_corrections",
        0,
        Some(&idem),
        Some(&params),
    )
    .await
    {
        Ok(job) => (
            StatusCode::CREATED,
            Json(json!({
                "status": "staged",
                "action_id": action_id,
                "pending_until_next_reconcile": false,
                "apply_job": job.id,
            })),
        )
            .into_response(),
        Err(e) => pg_err(e),
    }
}

/// `POST /v1/orgs/{org}/records/suppress` — exclude a record from assembly.
async fn suppress_record_route(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<SuppressIn>,
) -> impl IntoResponse {
    record_overlay_action(principal, state, org, body, "suppress", false).await
}

/// `POST /v1/orgs/{org}/records/unsuppress` — retract a record suppression.
async fn unsuppress_record_route(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<SuppressIn>,
) -> impl IntoResponse {
    record_overlay_action(principal, state, org, body, "suppress", true).await
}

/// `POST /v1/orgs/{org}/records/protect` — pin a record master for its entity.
async fn protect_record_route(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<SuppressIn>,
) -> impl IntoResponse {
    record_overlay_action(principal, state, org, body, "protect", false).await
}

/// `POST /v1/orgs/{org}/records/unprotect` — retract a record protection.
async fn unprotect_record_route(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<SuppressIn>,
) -> impl IntoResponse {
    record_overlay_action(principal, state, org, body, "protect", true).await
}

/// Steward+: stage an assertion retraction
/// (`DELETE /v1/orgs/{org}/assertions/{assertion_id}`).
async fn retract_assertion(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, assertion_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "steward actions require Postgres",
        );
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let action = json!({ "type": "retract_assertion", "assertion_id": assertion_id });
    match steward_stage(&client, &principal.actor, &org, &action).await {
        Ok(id) => staged_response(StatusCode::OK, &id),
        Err(r) => *r,
    }
}

/// The canonical (lexically ordered) form of a record pair, matching the lake's
/// assertion storage and `er_cli::canon`.
fn canon_pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// Lift `read::active_assertions` rows `(id, a, b, kind)` into er-reconcile
/// `Assertion`s (all active; rows with an unknown kind are dropped).
fn active_to_assertions(
    rows: Vec<(String, String, String, String)>,
) -> Vec<er_reconcile::assertions::Assertion> {
    rows.into_iter()
        .filter_map(|(id, a, b, kind)| {
            let kind = match kind.as_str() {
                "always" => er_reconcile::assertions::AssertionKind::Always,
                "never" => er_reconcile::assertions::AssertionKind::Never,
                _ => return None,
            };
            Some(er_reconcile::assertions::Assertion {
                assertion_id: id,
                rec_a_key: a,
                rec_b_key: b,
                kind,
                active: true,
            })
        })
        .collect()
}

/// Viewer+: the unsatisfiable rule sets (`GET /v1/orgs/{org}/assertions:contradictions`,
/// normalized to `/assertions/contradictions`): every active `never` whose
/// endpoints fall inside one `always`-connected component (S4.4.1
/// `check_contradiction_1`). Returns `{ contradictions: [...] }` — empty when the
/// ledger is satisfiable or absent.
async fn assertions_contradictions(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match org_lake(&state, &org).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let snapshot = match read::latest_snapshot(&session) {
        Ok(s) => s,
        Err(e) => return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}")),
    };
    let active = match read::active_assertions(&session, snapshot) {
        Ok(a) => a,
        Err(e) => return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}")),
    };
    let findings = er_reconcile::assertions::check_contradiction_1(&active_to_assertions(active));
    let items: Vec<Value> = findings
        .into_iter()
        .map(|c| {
            json!({
                "rec_a_key": c.rec_a_key,
                "rec_b_key": c.rec_b_key,
                "never_assertion_id": c.never_assertion_id,
                "always_assertion_ids": c.always_assertion_ids,
                "component": c.component.into_iter().collect::<Vec<_>>(),
            })
        })
        .collect();
    Json(json!({ "contradictions": items })).into_response()
}

#[derive(Deserialize)]
struct UnmergeIn {
    records: Vec<String>,
    #[serde(default = "default_true")]
    apply_now: bool,
}

fn default_true() -> bool {
    true
}

/// Steward+: split members out of a resolved entity
/// (`POST /v1/orgs/{org}/golden-records/{entity_id}/unmerge`). `records` is the
/// member record_keys to peel off; the endpoint stages one `never` rule between
/// each and a representative member it keeps (the lexically smallest member not
/// being peeled), so the next reconcile severs them. With `apply_now` (the
/// default) it also enqueues that incremental rebuild.
///
/// It reads the entity's membership + active assertions from the org's lake to
/// pick the representative and to run the S4.4.1 contradiction precheck: if a
/// proposed `never` falls inside an existing `always`-closure the split is
/// impossible, so it refuses (409) rather than stage a self-cancelling rule.
///
/// Note: the BFF addresses this as `{entity_id}:unmerge` (the Python action
/// convention); like the other `:action` routes this server exposes the sub-path
/// form, pending the deploy-time repoint.
async fn unmerge_entity(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, entity_id)): Path<(String, String)>,
    Json(body): Json<UnmergeIn>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    if body.records.is_empty() || body.records.len() > 100 {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "records[] must carry 1..=100 member record_keys",
        );
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "steward actions require Postgres",
        );
    };

    // Read the entity + active assertions from the lake (read-only) to pick the
    // kept representative and precheck the proposed never-rules.
    let session = match org_lake(&state, &org).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let snapshot = match read::latest_snapshot(&session) {
        Ok(s) => s,
        Err(e) => return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}")),
    };
    let members = match read::entity_member_keys(&session, snapshot, &entity_id) {
        Ok(m) => m,
        Err(e) => return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}")),
    };
    if members.is_empty() {
        return detail_err(StatusCode::NOT_FOUND, format!("no entity '{entity_id}'"));
    }
    let member_set: BTreeSet<&str> = members.iter().map(String::as_str).collect();
    // The requested records that are actually members, de-duplicated + ordered.
    let marked: BTreeSet<String> = body
        .records
        .iter()
        .filter(|r| member_set.contains(r.as_str()))
        .cloned()
        .collect();
    if marked.is_empty() {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "none of records[] are members of this entity",
        );
    }
    if marked.len() >= members.len() {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "cannot unmerge every member; keep at least one record in the entity",
        );
    }
    // Representative = the smallest member being kept (members is ascending).
    let representative = members
        .iter()
        .find(|m| !marked.contains(m.as_str()))
        .expect("a kept member exists (marked is a strict subset)");
    // One canonical never-pair per peeled record, against the representative.
    let proposed: Vec<(String, String)> = marked
        .iter()
        .map(|m| canon_pair(m, representative))
        .collect();

    // Contradiction-1 precheck over the active ledger + the proposed nevers.
    let active = match read::active_assertions(&session, snapshot) {
        Ok(a) => a,
        Err(e) => return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}")),
    };
    let mut assertions = active_to_assertions(active);
    for (i, (a, b)) in proposed.iter().enumerate() {
        assertions.push(er_reconcile::assertions::Assertion {
            assertion_id: format!("proposed-{i}"),
            rec_a_key: a.clone(),
            rec_b_key: b.clone(),
            kind: er_reconcile::assertions::AssertionKind::Never,
            active: true,
        });
    }
    let blocking: Vec<_> = er_reconcile::assertions::check_contradiction_1(&assertions)
        .into_iter()
        .filter(|c| c.never_assertion_id.starts_with("proposed-"))
        .collect();
    if let Some(c) = blocking.first() {
        return detail_err(
            StatusCode::CONFLICT,
            format!(
                "unmerge conflicts with an always-rule: '{}' and '{}' are forced together",
                c.rec_a_key, c.rec_b_key
            ),
        );
    }
    drop(session);

    // Stage one never-rule per peeled record (attributed to the steward).
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let mut action_ids: Vec<String> = Vec::with_capacity(proposed.len());
    for (a, b) in &proposed {
        let action = json!({
            "type": "add_assertion",
            "kind": "never",
            "a": a,
            "b": b,
            "created_by": principal.actor,
            "note": format!("unmerge:{entity_id}"),
        });
        match steward_stage(&client, &principal.actor, &org, &action).await {
            Ok(id) => action_ids.push(id),
            Err(r) => return *r,
        }
    }
    let pairs = action_ids.len();
    if !body.apply_now {
        return (
            StatusCode::OK,
            Json(json!({
                "status": "staged",
                "pairs": pairs,
                "action_ids": action_ids,
                "pending_until_next_reconcile": true,
            })),
        )
            .into_response();
    }
    // apply_now: enqueue the steward drain — apply the staged never-assertions
    // into the lake, then correct (re-reconcile over the existing scores).
    let idem = format!("unmerge-apply:{}", er_ids::new_v7());
    let params = json!({ "skip_ingest": true });
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    match pg::enqueue(
        &client,
        &org_row.id,
        "apply_corrections",
        0,
        Some(&idem),
        Some(&params),
    )
    .await
    {
        Ok(job) => (
            StatusCode::OK,
            Json(json!({
                "status": "applied",
                "pairs": pairs,
                "action_ids": action_ids,
                "apply_job": job.id,
            })),
        )
            .into_response(),
        Err(e) => pg_err(e),
    }
}

/// Viewer+: how many steward actions await the next reconcile
/// (`GET /v1/orgs/{org}/staged`).
async fn staged_count(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "steward actions require Postgres",
        );
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    match pg::pending_count(&client, &org_row.id).await {
        Ok(n) => Json(json!({ "pending_count": n })).into_response(),
        Err(e) => pg_err(e),
    }
}

#[derive(Deserialize)]
struct ReviewResolveIn {
    resolution: String,
}

/// Steward+: stage a review resolution
/// (`POST /v1/orgs/{org}/reviews/{review_id}/resolve`; the Python
/// `{review_id}:resolve` action). `resolution` is match | no_match | dismiss.
async fn resolve_review(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, review_id)): Path<(String, String)>,
    Json(body): Json<ReviewResolveIn>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    if !matches!(body.resolution.as_str(), "match" | "no_match" | "dismiss") {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "resolution must be match | no_match | dismiss",
        );
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "steward actions require Postgres",
        );
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let action = json!({
        "type": "resolve_review",
        "review_id": review_id,
        "resolution": body.resolution,
    });
    match steward_stage(&client, &principal.actor, &org, &action).await {
        Ok(id) => staged_response(StatusCode::OK, &id),
        Err(r) => *r,
    }
}

#[derive(Deserialize)]
struct BulkResolveItem {
    review_id: String,
    resolution: String,
}

#[derive(Deserialize)]
struct BulkResolveIn {
    items: Vec<BulkResolveItem>,
    #[serde(default)]
    apply_now: bool,
}

/// Steward+: stage many review resolutions in one window
/// (`POST /v1/orgs/{org}/reviews/bulk-resolve`). 1..=200 items; with `apply_now`
/// also enqueues the drain job.
async fn bulk_resolve_reviews(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<BulkResolveIn>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    if body.items.is_empty() || body.items.len() > 200 {
        return detail_err(StatusCode::UNPROCESSABLE_ENTITY, "items must be 1..=200");
    }
    if !body
        .items
        .iter()
        .all(|i| matches!(i.resolution.as_str(), "match" | "no_match" | "dismiss"))
    {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "resolution must be match | no_match | dismiss",
        );
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "steward actions require Postgres",
        );
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    // One active-org check for the whole batch.
    let org_row = match pg::get_org(&client, &org).await {
        Ok(Some(o)) if o.state == "active" => o,
        Ok(Some(o)) => {
            return detail_err(
                StatusCode::CONFLICT,
                format!(
                    "org '{org}' is {}; steward actions are accepted only when active",
                    o.state
                ),
            );
        }
        Ok(None) => return detail_err(StatusCode::NOT_FOUND, format!("no org '{org}'")),
        Err(e) => return pg_err(e),
    };
    let mut action_ids = Vec::new();
    for item in &body.items {
        let action = json!({
            "type": "resolve_review",
            "review_id": item.review_id,
            "resolution": item.resolution,
        });
        match pg::stage_action(&client, &org_row.id, &action, &principal.actor).await {
            Ok(id) => action_ids.push(id),
            Err(e) => return pg_err(e),
        }
    }
    let _ = pg::audit(
        &client,
        &principal.actor,
        Some(&org),
        "steward.bulk_resolve",
        &json!({ "count": action_ids.len() }),
    )
    .await;
    let mut resp = json!({ "status": "staged", "action_ids": action_ids });
    if body.apply_now {
        let idem = format!("bulk-resolve-apply:{}", er_ids::new_v7());
        let params = json!({ "skip_ingest": true });
        match pg::enqueue(
            &client,
            &org_row.id,
            "apply_corrections",
            0,
            Some(&idem),
            Some(&params),
        )
        .await
        {
            Ok(job) => resp["apply_job"] = json!(job.id),
            Err(e) => return pg_err(e),
        }
    }
    (StatusCode::OK, Json(resp)).into_response()
}

/// The 11 overridable golden columns (er-golden SCALAR + ADDRESS); the mass-update
/// API only accepts these.
const OVERRIDABLE_ATTRS: [&str; 11] = [
    "given_name",
    "family_name",
    "email",
    "phone_e164",
    "birth_date",
    "addr_number",
    "addr_street",
    "addr_unit",
    "addr_city",
    "addr_region",
    "addr_postal",
];

#[derive(Deserialize)]
struct MassUpdateItem {
    entity_id: String,
    attribute: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    clear: bool,
}

#[derive(Deserialize)]
struct MassUpdateIn {
    items: Vec<MassUpdateItem>,
    #[serde(default)]
    apply_now: bool,
}

/// Steward+: stage golden field overrides across many entities in one window
/// (`POST /v1/orgs/{org}/mass-update`). 1..=200 items; each sets (`value`) or
/// clears (`clear: true`) one golden field for one entity. With `apply_now` also
/// enqueues the drain (`apply_corrections`), which re-asssembles golden over the
/// staged overrides. Durable + auditable; the matcher/scores are untouched.
async fn mass_update(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<MassUpdateIn>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    if body.items.is_empty() || body.items.len() > 200 {
        return detail_err(StatusCode::UNPROCESSABLE_ENTITY, "items must be 1..=200");
    }
    if !body
        .items
        .iter()
        .all(|i| !i.entity_id.is_empty() && OVERRIDABLE_ATTRS.contains(&i.attribute.as_str()))
    {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "each item needs an entity_id and an attribute in the golden field set",
        );
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "steward actions require Postgres",
        );
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match pg::get_org(&client, &org).await {
        Ok(Some(o)) if o.state == "active" => o,
        Ok(Some(o)) => {
            return detail_err(
                StatusCode::CONFLICT,
                format!(
                    "org '{org}' is {}; steward actions are accepted only when active",
                    o.state
                ),
            );
        }
        Ok(None) => return detail_err(StatusCode::NOT_FOUND, format!("no org '{org}'")),
        Err(e) => return pg_err(e),
    };
    let mut action_ids = Vec::new();
    for item in &body.items {
        let action = if item.clear {
            json!({
                "type": "clear_field",
                "entity_id": item.entity_id,
                "attribute": item.attribute,
            })
        } else {
            json!({
                "type": "override_field",
                "entity_id": item.entity_id,
                "attribute": item.attribute,
                "value": item.value,
            })
        };
        match pg::stage_action(&client, &org_row.id, &action, &principal.actor).await {
            Ok(id) => action_ids.push(id),
            Err(e) => return pg_err(e),
        }
    }
    let _ = pg::audit(
        &client,
        &principal.actor,
        Some(&org),
        "steward.mass_update",
        &json!({ "count": action_ids.len() }),
    )
    .await;
    let mut resp = json!({ "status": "staged", "action_ids": action_ids });
    if body.apply_now {
        let idem = format!("mass-update-apply:{}", er_ids::new_v7());
        let params = json!({ "skip_ingest": true });
        match pg::enqueue(
            &client,
            &org_row.id,
            "apply_corrections",
            0,
            Some(&idem),
            Some(&params),
        )
        .await
        {
            Ok(job) => resp["apply_job"] = json!(job.id),
            Err(e) => return pg_err(e),
        }
    }
    (StatusCode::OK, Json(resp)).into_response()
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
            Json(json!({ "detail": format!("invalid role '{}'", body.role) })),
        )
            .into_response();
    };
    let Some(dsn) = &state.dsn else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({ "detail": "api keys require Postgres" })),
        )
            .into_response();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    // History images record the acting principal (er.actor).
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::issue_key(&client, &org_row.id, role.as_str()).await {
        Ok((key_id, key)) => {
            let _ = pg::audit(
                &client,
                &principal.actor,
                Some(&org),
                "key.issue",
                &json!({ "key_id": key_id, "role": role.as_str() }),
            )
            .await;
            (
                StatusCode::CREATED,
                Json(json!({ "id": key_id, "key": key, "role": role.as_str() })),
            )
                .into_response()
        }
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
            Json(json!({ "detail": "api keys require Postgres" })),
        )
            .into_response();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    // History images record the acting principal (er.actor).
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::revoke_key(&client, &key_id, &org_row.id).await {
        Ok(revoked) => {
            let _ = pg::audit(
                &client,
                &principal.actor,
                Some(&org),
                "key.revoke",
                &json!({ "key_id": key_id, "revoked": revoked }),
            )
            .await;
            (StatusCode::OK, Json(json!({ "revoked": revoked }))).into_response()
        }
        Err(e) => pg_err(e),
    }
}

/// Build the control-plane router. `/healthz` is anonymous; every `/v1` route is
/// behind the bearer boundary.
///
/// Action endpoints are registered in the `/parent/{id}/action` sub-path form
/// (axum/matchit forbids a `{param}:action` segment); [`app`] wraps this with a
/// normalizer that also accepts the BFF's `{id}:action` spelling.
fn build_routes(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/orgs", get(list_orgs).post(create_org))
        .route("/v1/email", post(send_email))
        .route("/v1/jobs", get(jobs_global))
        .route("/v1/audit", get(audit_global))
        .route("/v1/orgs/{org}", get(get_org))
        .route("/v1/orgs/{org}/audit", get(audit_index))
        .route(
            "/v1/orgs/{org}/resources",
            axum::routing::patch(update_resources),
        )
        // Python's `{org}:suspend` / `{org}:resume` actions become sub-paths
        // (matchit forbids a param+literal in one segment, as with job cancel).
        .route("/v1/orgs/{org}/suspend", post(suspend_org))
        .route("/v1/orgs/{org}/resume", post(resume_org))
        .route("/v1/orgs/{org}/jobs", post(create_job).get(list_jobs))
        .route("/v1/orgs/{org}/jobs/{job_id}", get(get_job))
        // axum/matchit requires a param to be a whole segment, so the Python
        // `{job_id}:cancel` action becomes a `/cancel` sub-path here.
        .route("/v1/orgs/{org}/jobs/{job_id}/cancel", post(cancel_job))
        .route("/v1/orgs/{org}/jobs/{job_id}/resume", post(resume_job))
        .route("/v1/orgs/{org}/api-keys", post(create_key))
        .route(
            "/v1/orgs/{org}/api-keys/{key_id}",
            axum::routing::delete(revoke_key),
        )
        .route(
            "/v1/orgs/{org}/config/versions",
            post(create_config_version).get(list_config_versions),
        )
        .route(
            "/v1/orgs/{org}/config/versions/{version}",
            get(get_config_version),
        )
        // Python's `{version}:publish` action -> a `/publish` sub-path (matchit).
        .route(
            "/v1/orgs/{org}/config/versions/{version}/publish",
            post(publish_config_version),
        )
        .route("/v1/orgs/{org}/config", get(active_config))
        .route("/v1/orgs/{org}/lexicon", get(get_lexicon).put(put_lexicon))
        .route("/v1/orgs/{org}/golden-records", get(golden_records_list))
        .route(
            "/v1/orgs/{org}/golden-records/{entity_id}",
            get(golden_record_detail),
        )
        // Python's `{entity_id}:unmerge` action -> an `/unmerge` sub-path (matchit).
        .route(
            "/v1/orgs/{org}/golden-records/{entity_id}/unmerge",
            post(unmerge_entity),
        )
        .route("/v1/orgs/{org}/metrics", get(org_metrics))
        .route("/v1/orgs/{org}/data-quality", get(org_data_quality))
        .route("/v1/orgs/{org}/duplicates", get(duplicates_list))
        .route("/v1/orgs/{org}/match-scores", get(match_scores_list))
        .route("/v1/orgs/{org}/match-preview", get(match_preview))
        .route("/v1/orgs/{org}/records", get(records_by_key))
        .route("/v1/orgs/{org}/reviews", get(reviews_list))
        .route(
            "/v1/orgs/{org}/reviews/bulk-resolve",
            post(bulk_resolve_reviews),
        )
        .route("/v1/orgs/{org}/mass-update", post(mass_update))
        .route(
            "/v1/orgs/{org}/records/suppress",
            post(suppress_record_route),
        )
        .route(
            "/v1/orgs/{org}/records/unsuppress",
            post(unsuppress_record_route),
        )
        .route("/v1/orgs/{org}/records/protect", post(protect_record_route))
        .route(
            "/v1/orgs/{org}/records/unprotect",
            post(unprotect_record_route),
        )
        .route("/v1/orgs/{org}/reviews/{review_id}", get(review_detail))
        .route(
            "/v1/orgs/{org}/reviews/{review_id}/resolve",
            post(resolve_review),
        )
        .route(
            "/v1/orgs/{org}/assertions",
            get(assertions_list).post(add_assertion),
        )
        // Python's `assertions:contradictions` -> a `/contradictions` sub-path.
        .route(
            "/v1/orgs/{org}/assertions/contradictions",
            get(assertions_contradictions),
        )
        .route(
            "/v1/orgs/{org}/assertions/{assertion_id}",
            axum::routing::delete(retract_assertion),
        )
        .route("/v1/orgs/{org}/staged", get(staged_count))
        .route("/v1/orgs/{org}/imports", get(imports_list))
        .route("/v1/orgs/{org}/runs", get(runs_list))
        .route("/v1/orgs/{org}/merge-plans", get(merge_plans))
        .route(
            "/v1/orgs/{org}/schedules",
            get(schedules_index).post(create_schedule),
        )
        .route(
            "/v1/orgs/{org}/webhooks",
            get(webhooks_index).post(create_webhook),
        )
        .route(
            "/v1/orgs/{org}/webhooks/{webhook_id}",
            axum::routing::delete(delete_webhook),
        )
        .route(
            "/v1/orgs/{org}/schedules/{schedule_id}",
            axum::routing::patch(update_schedule).delete(delete_schedule),
        )
        .with_state(state)
}

/// The action verbs the BFF appends as `{id}:action`, each mapping to a
/// `{id}/action` sub-path route on this server.
const ACTION_VERBS: [&str; 8] = [
    "unmerge",
    "resolve",
    "cancel",
    "resume",
    "publish",
    "bulk-resolve",
    "contradictions",
    "suspend",
];

/// Rewrite a Python-style `{id}:action` path into the `{id}/action` sub-path this
/// server routes on. Splits the LAST segment on its LAST `:` and only rewrites
/// when the suffix is one of [`ACTION_VERBS`] — so an id that itself contains
/// colons (an unencoded `review_id` like `gray_band:crm:C001:webforms:W006`) is
/// left alone, while the BFF's encoded `{id}:action` (the id's own colons arrive
/// as `%3A`) is normalized. Returns `None` when no rewrite applies.
fn rewrite_action_path(path: &str) -> Option<String> {
    let (prefix, last) = path.rsplit_once('/')?;
    let (head, action) = last.rsplit_once(':')?;
    if !ACTION_VERBS.contains(&action) {
        return None;
    }
    Some(format!("{prefix}/{head}/{action}"))
}

/// The control-plane service: `build_routes`, wrapped so the BFF's
/// `{id}:action` spelling is accepted alongside the native `{id}/action`
/// sub-path. The rewrite must happen BEFORE routing (an unknown `:action` path
/// matches no route), so it sits in an outer service that normalizes the URI and
/// defers to the inner router — not a `Router::layer`, which runs post-routing.
pub fn app(state: AppState) -> Router {
    let inner = build_routes(state);
    Router::new().fallback_service(tower::service_fn(move |mut req: Request<Body>| {
        let inner = inner.clone();
        async move {
            if let Some(new_path) = rewrite_action_path(req.uri().path()) {
                let pq = match req.uri().query() {
                    Some(q) => format!("{new_path}?{q}"),
                    None => new_path,
                };
                if let Ok(pq) = pq.parse::<axum::http::uri::PathAndQuery>() {
                    let mut parts = req.uri().clone().into_parts();
                    parts.path_and_query = Some(pq);
                    if let Ok(uri) = axum::http::Uri::from_parts(parts) {
                        *req.uri_mut() = uri;
                    }
                }
            }
            // Router's Service error is Infallible, so this never returns Err.
            inner.oneshot(req).await
        }
    }))
}

/// Build the server state from the environment: the operator bearer
/// (`ER_OPERATOR_TOKEN`) and, if `ER_PG_DSN` is set, the Postgres job-queue
/// backend (otherwise the in-memory store).
#[must_use]
pub async fn state_from_env() -> AppState {
    let operator = std::env::var("ER_OPERATOR_TOKEN").unwrap_or_default();
    let state = match std::env::var("ER_PG_DSN") {
        Ok(dsn) if !dsn.is_empty() => AppState::with_pg(operator, dsn),
        _ => AppState::new(operator),
    };
    // Async because a SES backend resolves credentials at construction; `off`
    // and `log` do not await anything.
    state.with_mail(mail::Mailer::from_env().await)
}

#[cfg(test)]
mod tests {
    use super::rewrite_action_path;

    #[test]
    fn rewrites_every_action_spelling_the_bff_uses() {
        let cases = [
            (
                "/v1/orgs/acme/golden-records/E1:unmerge",
                "/v1/orgs/acme/golden-records/E1/unmerge",
            ),
            (
                "/v1/orgs/acme/reviews/r1:resolve",
                "/v1/orgs/acme/reviews/r1/resolve",
            ),
            (
                "/v1/orgs/acme/jobs/J1:cancel",
                "/v1/orgs/acme/jobs/J1/cancel",
            ),
            (
                "/v1/orgs/acme/jobs/J1:resume",
                "/v1/orgs/acme/jobs/J1/resume",
            ),
            (
                "/v1/orgs/acme/config/versions/3:publish",
                "/v1/orgs/acme/config/versions/3/publish",
            ),
            (
                "/v1/orgs/acme/reviews:bulk-resolve",
                "/v1/orgs/acme/reviews/bulk-resolve",
            ),
            (
                "/v1/orgs/acme/assertions:contradictions",
                "/v1/orgs/acme/assertions/contradictions",
            ),
            ("/v1/orgs/acme:suspend", "/v1/orgs/acme/suspend"),
            ("/v1/orgs/acme:resume", "/v1/orgs/acme/resume"),
        ];
        for (input, want) in cases {
            assert_eq!(rewrite_action_path(input).as_deref(), Some(want), "{input}");
        }
    }

    #[test]
    fn leaves_a_percent_encoded_id_colon_intact() {
        // The BFF percent-encodes ids, so `crm:C001` arrives as `crm%3AC001`; the
        // only literal `:` is the action delimiter.
        assert_eq!(
            rewrite_action_path("/v1/orgs/acme/golden-records/crm%3AC001:unmerge").as_deref(),
            Some("/v1/orgs/acme/golden-records/crm%3AC001/unmerge"),
        );
    }

    #[test]
    fn leaves_non_action_paths_unchanged() {
        for p in [
            "/healthz",
            "/v1/orgs",
            "/v1/orgs/acme/jobs/J1",
            "/v1/orgs/acme/golden-records/E1",
            // An unencoded review_id full of colons, with NO trailing action verb:
            // the last `:` suffix (`W006`) isn't an action, so it's left intact.
            "/v1/orgs/acme/reviews/gray_band:crm:C001:webforms:W006",
            // A sub-path action is already normalized — unchanged.
            "/v1/orgs/acme/reviews/gray_band:crm:C001:webforms:W006/resolve",
        ] {
            assert_eq!(rewrite_action_path(p), None, "{p}");
        }
    }

    #[test]
    fn rewrites_an_action_on_an_id_that_itself_has_colons() {
        // The BFF encodes the review_id, so its colons arrive as %3A and the only
        // bare `:` is the action delimiter — the rewrite peels just the action.
        assert_eq!(
            rewrite_action_path(
                "/v1/orgs/acme/reviews/gray_band%3Acrm%3AC001%3Awebforms%3AW006:resolve"
            )
            .as_deref(),
            Some("/v1/orgs/acme/reviews/gray_band%3Acrm%3AC001%3Awebforms%3AW006/resolve"),
        );
    }
}
