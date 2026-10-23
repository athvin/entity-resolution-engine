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
pub mod cron;
pub mod dispatcher;
pub mod errfmt;
pub mod mail;
pub mod openapi;
pub mod pg;
pub mod provision;
pub mod read;
pub mod schema;
pub mod urlguard;
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
    /// The `POST /v1/orgs/{org}/imports` body cap, enforced both as the route's
    /// body limit and while the part streams. Defaults to
    /// [`DEFAULT_MAX_IMPORT_BYTES`]; configurable so an operator can tighten it
    /// and so the oversize path is testable without a 256 MiB request.
    max_import_bytes: usize,
    /// The tenant auto-provisioning target (D8), or `None` when provisioning is
    /// off — in which case `POST /v1/orgs` without a `config_path` keeps
    /// answering 503, exactly as it did before provisioning existed.
    ///
    /// Held on the state rather than read from the environment inside the
    /// handler, for the same reason `mail` is: a test needs to construct one
    /// directly. Setting process env from a test is not an option here —
    /// `std::env::set_var` is `unsafe` in edition 2024 and the workspace denies
    /// `unsafe_code`.
    provision: Option<provision::Profile>,
    /// Set when provisioning IS configured but configured wrongly. Kept apart
    /// from `None` on purpose: an operator who opted in and made a typo should
    /// see which variable is wrong, not a 503 that says provisioning is off.
    provision_error: Option<String>,
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
            max_import_bytes: DEFAULT_MAX_IMPORT_BYTES,
            provision: None,
            provision_error: None,
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
            max_import_bytes: DEFAULT_MAX_IMPORT_BYTES,
            provision: None,
            provision_error: None,
        }
    }

    /// Attach a mail backend. Separate from the constructors because building a
    /// SES client is async and these are not.
    #[must_use]
    pub fn with_mail(mut self, mailer: mail::Mailer) -> Self {
        self.mail = Arc::new(mailer);
        self
    }

    /// Set the tenant auto-provisioning target. `Ok(None)` leaves provisioning
    /// off; `Err` records the misconfiguration so the handler can name it.
    #[must_use]
    pub fn with_provision(
        mut self,
        profile: Result<Option<provision::Profile>, provision::ProfileError>,
    ) -> Self {
        match profile {
            Ok(p) => self.provision = p,
            Err(e) => self.provision_error = Some(e.to_string()),
        }
        self
    }

    /// Override the import body cap: the `POST /v1/orgs/{org}/imports` route's
    /// body limit and the running byte count while the part streams.
    #[must_use]
    pub fn with_max_import_bytes(mut self, bytes: usize) -> Self {
        self.max_import_bytes = bytes;
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

/// Refuse an operation on a tenant that is not `active`, as a 409 naming the
/// state it is in. `what` is the plural noun for the thing being refused
/// ("jobs", "assertions") so the message reads like the imports one.
///
/// Every handler that ENQUEUES work has to call this. The guarantee is stated in
/// the tenant-admin UI — "a suspended tenant refuses jobs, imports and steward
/// actions until resumed" — and it was only partly true. Four enqueue paths
/// already refused, each in its own idiom: `delivery_preconditions` compares
/// `org_row.state`, `publish_config_version` compares a separately-fetched
/// `org_state`, and `put_lexicon` uses a match guard
/// (`Ok(Some(o)) if o.state == "active"`). Three did not refuse at all —
/// `create_job`, `add_assertion` and `record_overlay_action` went role-check →
/// org-exists → `pg::enqueue` — so a suspended tenant could be made to spend
/// compute and mutate its own lake through any of those three doors.
///
/// A shared helper rather than a fourth hand-written spelling, because four
/// idioms for one rule is precisely how the next handler gets forgotten. The
/// existing three are left alone: each is load-bearing where it sits (publish
/// needs its check atomic with the version repoint), and rewriting them would
/// put behaviour changes in a bug-fix diff.
///
/// Boxed like `org_or_404`'s error, because an axum `Response` is large enough
/// that clippy's `result_large_err` refuses it unboxed.
fn refuse_unless_active(
    org: &pg::OrgDetail,
    what: &str,
) -> Result<(), Box<axum::response::Response>> {
    if org.state == "active" {
        return Ok(());
    }
    Err(Box::new(detail_err(
        StatusCode::CONFLICT,
        format!(
            "org '{}' is {}; {what} are accepted only when active",
            org.name, org.state
        ),
    )))
}

/// The org-env key naming the config file the engine reads. It duplicates the
/// `orgs.config_path` column, and `create_org` keeps the two in agreement.
const ER_CONFIG_PATH_KEY: &str = "ER_CONFIG_PATH";

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

/// The header the BFF uses to name the human behind a request. It has been sent
/// on every forwarded call since the BFF was written — `proxy.ts` sets
/// `"x-acting-user": access.identity.user.email` — and its own comment states
/// the intended result: "erserver's audit_log records `user:<email> via key:…`".
/// Nothing here read it, so every human action was attributed to the service
/// credential instead. This is the receiving half.
const ACTING_USER_HEADER: &str = "x-acting-user";

/// The longest acting-user we will record. An email, not an essay; the value
/// lands in `audit_log.actor` and `assertions.created_by`.
const ACTING_USER_MAX: usize = 128;

/// The human named by [`ACTING_USER_HEADER`], if the header is present and
/// sane. `None` leaves the actor as the credential alone.
///
/// **This is attribution, never authorization.** `Principal.org` and
/// `Principal.role` keep coming solely from the bearer credential, so this
/// header cannot widen what a caller may do — only what the audit trail calls
/// them. That split is the whole safety property: were `role` to read this, the
/// header would be an impersonation vector.
///
/// The honest limit: anyone holding a valid key can set it, so attribution is
/// exactly as trustworthy as the key. It is still a strict improvement on
/// `key:<id>`, and because the credential is recorded ALONGSIDE the person
/// (`user:… via key:…`) the trail keeps its provenance — a forged name cannot
/// hide which credential carried the request. Making attribution unforgeable
/// needs a credential the tenant does not hold, which is the service-identity
/// work (#78), not this.
///
/// Rejected values are ignored rather than refused: a malformed header should
/// not fail an otherwise valid request, and falling back to the credential form
/// loses nothing relative to the previous behaviour.
fn forwarded_actor(parts: &Parts) -> Option<String> {
    let raw = parts.headers.get(ACTING_USER_HEADER)?.to_str().ok()?.trim();
    acceptable_actor(raw).then(|| raw.to_string())
}

/// Whether a trimmed acting-user is fit to record: printable, non-space ASCII,
/// bounded, and free of the two characters that would need escaping wherever the
/// actor is embedded.
///
/// Separate from [`forwarded_actor`] so the rule is testable without a header,
/// which matters because the `http` crate refuses to CONSTRUCT a header value
/// containing `\n` or `\r` at all — CRLF injection is already impossible one
/// layer below this, and the check here is defence in depth rather than the
/// only thing standing in the way.
fn acceptable_actor(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= ACTING_USER_MAX
        && raw
            .chars()
            .all(|c| c.is_ascii_graphic() && c != '"' && c != '\\')
}

/// Compose the recorded actor from the credential and the forwarded human.
/// `user:<email> via <credential>` is the format `proxy.ts` documents.
fn compose_actor(credential: &str, user: Option<String>) -> String {
    match user {
        Some(user) => format!("user:{user} via {credential}"),
        None => credential.to_string(),
    }
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
                actor: compose_actor("operator", forwarded_actor(parts)),
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
                    // Role and org come from the KEY, never from the header.
                    actor: compose_actor(&format!("key:{key_id}"), forwarded_actor(parts)),
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

/// Auto-provision a tenant from the declared profile (D8).
///
/// Synchronous half: derive the tenant's paths, record the org as
/// `provisioning`, land the starter config as version 1 and publish it, mint the
/// customer's one-time admin key, and enqueue the `provision` job that does the
/// slow work. Returns the shape the BFF and the Playwright mock already agree
/// on — `{name, tenant, config_version, state, job_id, admin_key, admin_key_id}`.
///
/// Why the split. The lake is created by the JOB, not here: creating a DuckLake
/// (or a Postgres catalog database) takes long enough that holding the request
/// open for it is the wrong shape, and the UI was built to poll — a progress
/// panel that waits for `provisioning` to become `active`, with a 180s budget in
/// the e2e spec. The state enum, the mock, the registry's `provision_job_id` and
/// that poller were all already here; only the server end was missing.
///
/// The admin key is returned ONCE and never stored in plaintext — `issue_key`
/// stores a hash, and this response is the only time the secret exists outside
/// the caller's hands.
async fn provision_org(
    principal: &Principal,
    dsn: &str,
    profile: &provision::Profile,
    name: &str,
) -> axum::response::Response {
    let plan = profile.plan(name);
    // Read the starter config BEFORE touching the database: a missing or
    // unreadable template is an operator error, and discovering it after the org
    // row exists would leave a half-provisioned tenant behind.
    let yaml = match provision::starter_config(&profile.config_template) {
        Ok(yaml) => yaml,
        Err(e) => return detail_err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    // And validate it, for the same reason `create_config_version` does: a
    // starter config that cannot load would strand every tenant provisioned
    // from this profile.
    let config = match load_config_str(&yaml) {
        Ok(c) => c,
        Err(e) => {
            return detail_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!(
                    "the starter config {} does not load: {}",
                    profile.config_template.display(),
                    errfmt::chain(&e)
                ),
            );
        }
    };
    let problems = config_check::config_reference_problems(&config);
    if !problems.is_empty() {
        return detail_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "the starter config {} references unknown columns: {}",
                profile.config_template.display(),
                problems.join("; ")
            ),
        );
    }

    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let _ = pg::set_actor(&client, &principal.actor).await;

    let env = plan.env_json();
    if let Err(e) = pg::upsert_org_byo(
        &client,
        name,
        &plan.config_path.to_string_lossy(),
        &env,
        Some(&plan.drop_root.to_string_lossy()),
    )
    .await
    {
        return pg_err(e);
    }
    // `upsert_byo` inserts `active`; a provisioned tenant is not usable until
    // its lake exists, so step it back to `provisioning` and let the job
    // promote it. Expected-state guarded, so a replay of this call on an
    // already-active tenant leaves it alone.
    let _ = pg::set_org_state(&client, name, "provisioning", "active").await;

    let org_row = match pg::get_org(&client, name).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            return detail_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "org vanished after upsert",
            );
        }
        Err(e) => return pg_err(e),
    };

    // The starter config as version 1, published so the tenant has an active
    // config from the start — the wizard and the upload path both require one.
    let hash = config_hash(&config);
    let version =
        match pg::create_config_version(&client, &org_row.id, &yaml, &hash, &principal.actor).await
        {
            Ok(v) => v,
            Err(e) => return pg_err(e),
        };
    if let Err(e) = provision::create_workspace(&plan, &profile.config_template) {
        return detail_err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    // (org_id, org_name) — the two are adjacent `&str`s, so a swap compiles and
    // silently publishes nothing. Caught by asserting `active_config_version`.
    if let Err(e) = pg::publish_config_version(&client, &org_row.id, name, version, Some("C")).await
    {
        return pg_err(e);
    }

    let (admin_key_id, admin_key) = match pg::issue_key(&client, &org_row.id, "admin").await {
        Ok(pair) => pair,
        Err(e) => return pg_err(e),
    };

    let idem = format!("provision:{name}");
    let job = match pg::enqueue(
        &client,
        &org_row.id,
        "provision",
        0,
        Some(&idem),
        None,
        Some(&principal.actor),
    )
    .await
    {
        Ok(job) => job,
        Err(e) => return pg_err(e),
    };
    let _ = pg::audit(
        &client,
        &principal.actor,
        Some(name),
        "org.provision",
        &json!({ "job_id": job.id, "mode": format!("{:?}", profile.mode).to_lowercase() }),
    )
    .await;

    (
        StatusCode::CREATED,
        Json(json!({
            "name": name,
            "tenant": plan.tenant,
            "config_version": version,
            "state": "provisioning",
            "job_id": job.id,
            "admin_key_id": admin_key_id,
            "admin_key": admin_key,
        })),
    )
        .into_response()
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
        // Auto mode: provision from the declared profile. Unconfigured is still
        // the 503 — that branch was the only behaviour until D8, and keeping it
        // means an operator who has not opted in sees no change.
        return match (&state.provision, &state.provision_error) {
            (Some(profile), _) => provision_org(&principal, dsn, profile, &body.name).await,
            // Switched on and misconfigured: say which variable, rather than
            // pretending provisioning is simply off.
            (None, Some(e)) => detail_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("auto-provisioning is misconfigured: {e}"),
            ),
            (None, None) => detail_err(
                StatusCode::SERVICE_UNAVAILABLE,
                "auto-provisioning not configured (set ER_PROVISION_MODE)",
            ),
        };
    };
    // `config_path` and `env.ER_CONFIG_PATH` name the same file and are stored
    // independently, which was a silent-failure footgun: publish writes the
    // YAML to the COLUMN (`write_config_file(&config_path, …)`) while every run
    // reads the path out of ENV (`env_str(env, "ER_CONFIG_PATH")` in the
    // dispatcher). Point them at different files and the tenant's publishes
    // succeed, the config studio shows the new version live, and not one run
    // ever reads it — the same shape as E10a's tier-C publish that rebuilt
    // against the model it was meant to replace.
    //
    // One source of truth, enforced here rather than hoped for: inject the path
    // when the caller omitted it, and refuse when they supplied a different one.
    // Refusing beats silently overriding — a caller who passed two different
    // paths believes something we would otherwise quietly discard.
    let mut env_map = body.env;
    match env_map.get(ER_CONFIG_PATH_KEY) {
        None => {
            env_map.insert(ER_CONFIG_PATH_KEY.to_string(), config_path.to_string());
        }
        Some(from_env) if from_env != config_path => {
            return detail_err(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!(
                    "config_path '{config_path}' and env.{ER_CONFIG_PATH_KEY} '{from_env}' must \
                     name the same file: publish writes to config_path and every run reads \
                     env.{ER_CONFIG_PATH_KEY}, so a mismatch means published configs never take \
                     effect"
                ),
            );
        }
        Some(_) => {}
    }
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let env = json!(env_map);
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
            let seeded = seed_starter_model(&client, &body.name, &env, &principal.actor).await;
            (
                StatusCode::CREATED,
                Json(json!({ "name": body.name, "starter_model": seeded })),
            )
                .into_response()
        }
        Err(e) => pg_err(e),
    }
}

/// Seed the tenant's starter matching model into the registry from the model
/// file its env already points at, and activate it.
///
/// This is the matrix's "zero-config starter model" row, and it is deliberately
/// seeded from `ER_MODEL_PATH` rather than from a copy of the document: that is
/// the exact file the dispatcher used to read on every run, so a tenant created
/// after the registry exists scores against byte-identical settings to one
/// created before it. From here on, versions are managed through the API.
///
/// Best-effort by design. An org whose env names no model (or an unreadable
/// path) is still created — it simply has no registry model yet, and the
/// dispatcher falls back to the env file exactly as it always did. Returns what
/// happened, for the response body and the operator.
async fn seed_starter_model(
    client: &tokio_postgres::Client,
    org: &str,
    env: &Value,
    actor: &str,
) -> Value {
    let Some(path) = env.get("ER_MODEL_PATH").and_then(Value::as_str) else {
        return json!({ "seeded": false, "reason": "org env names no ER_MODEL_PATH" });
    };
    let Ok(model_json) = std::fs::read_to_string(path) else {
        return json!({ "seeded": false, "reason": format!("cannot read {path}") });
    };
    if dedupe_core::Settings::from_json_str(&model_json).is_err() {
        return json!({ "seeded": false, "reason": format!("{path} is not a loadable model") });
    }
    let Ok(Some(org_row)) = pg::get_org(client, org).await else {
        return json!({ "seeded": false, "reason": "org row not readable" });
    };
    // `POST /v1/orgs` is an idempotent upsert, so a replay must not mint a
    // second version: versions auto-increment, which would otherwise make
    // re-registering an org silently accumulate identical models.
    match pg::list_model_versions(client, &org_row.id).await {
        Ok(existing) if !existing.is_empty() => {
            return json!({
                "seeded": false,
                "reason": "the org already has model versions",
                "versions": existing.len(),
            });
        }
        Ok(_) => {}
        Err(e) => return json!({ "seeded": false, "reason": format!("{e}") }),
    }
    // The entity type the lake will resolve. No published config yet at
    // create-org time, so fall back to the env's declared type, then person.
    let entity_type = env
        .get("ER_ENTITY_TYPE")
        .and_then(Value::as_str)
        .unwrap_or("person");
    let provenance = json!({
        "origin": "seeded",
        "source_path": path,
        "model_version_env": env.get("ER_MODEL_VERSION"),
        "by": actor,
    });
    let created = pg::create_model_version(
        client,
        &org_row.id,
        pg::DEFAULT_MODEL_NAME,
        entity_type,
        &model_json,
        env.get("ER_MODEL_VERSION")
            .and_then(Value::as_str)
            .unwrap_or("v0001"),
        env.get("ER_TF_SNAPSHOT_ID").and_then(Value::as_str),
        &provenance,
        actor,
    )
    .await;
    match created {
        Ok(m) => {
            if let Err(e) = pg::activate_model_version(client, &org_row.id, &m.id).await {
                return json!({ "seeded": false, "reason": format!("activate failed: {e}") });
            }
            json!({
                "seeded": true,
                "name": m.name,
                "version": m.version,
                "model_hash": m.model_hash,
            })
        }
        // A re-POST of the same org (the upsert path) already has its seed.
        Err(e) => json!({ "seeded": false, "reason": format!("{e}") }),
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

/// The model registry's wire projection. `model_json` is omitted from list
/// responses — it is the largest field by far and a list of twenty versions
/// would be mostly settings documents nobody asked for.
fn model_summary(m: &er_entities::matching_model::MatchingModel) -> Value {
    json!({
        "id": m.id,
        "name": m.name,
        "version": m.version,
        "entity_type": m.entity_type,
        "model_hash": m.model_hash,
        "model_version_label": m.model_version_label,
        "tf_snapshot_id": m.tf_snapshot_id,
        "state": m.state,
        "provenance": m.provenance,
        "created_by": m.created_by,
        "activated_at": m.activated_at.as_ref().map(pg::ts),
        "created_at": pg::ts(&m.created_at),
    })
}

/// A model version's per-field weights: for each comparison, each level's
/// `m`/`u` probabilities and the log-odds they imply.
///
/// This is the matrix's "configurable field weights" surface. The engine has
/// always carried these numbers and could round-trip them, but nothing outside
/// it could read them — `grep m_probability` over the server and the frontend
/// returned nothing at all. Derived from the stored document rather than
/// duplicated into columns, so it cannot drift from what the engine loads.
fn model_weights(model_json: &str) -> Option<Value> {
    let settings = dedupe_core::settings::json::SettingsJson::from_json_str(model_json).ok()?;
    let comparisons: Vec<Value> = settings
        .comparisons
        .iter()
        .map(|c| {
            let levels: Vec<Value> = c
                .comparison_levels
                .iter()
                .map(|l| {
                    json!({
                        "label": l.label_for_charts,
                        "sql_condition": l.sql_condition,
                        "is_null_level": l.is_null_level,
                        "m_probability": l.m_probability,
                        "u_probability": l.u_probability,
                        // log2(m/u) is the weight the level contributes to a
                        // pair's score; shown so a reviewer can read the model
                        // without recomputing it.
                        "log2_bayes_factor": match (l.m_probability, l.u_probability) {
                            (Some(m), Some(u)) if u > 0.0 && m > 0.0 => Some((m / u).log2()),
                            _ => None,
                        },
                        "tf_adjustment_column": l.tf_adjustment_column,
                    })
                })
                .collect();
            json!({
                "output_column_name": c.output_column_name,
                "comparison_description": c.comparison_description,
                "levels": levels,
            })
        })
        .collect();
    Some(json!({
        "link_type": settings.link_type,
        "probability_two_random_records_match": settings.probability_two_random_records_match,
        "blocking_rules": settings.blocking_rules_to_generate_predictions,
        "comparisons": comparisons,
    }))
}

/// Admin+: compile the org's ACTIVE config into a new draft model version
/// (`POST /v1/orgs/{org}/models:compile`).
///
/// This is the endpoint that makes the tenant-facing matching config mean
/// something. The `comparisons:` / `blocking:` blocks have always been
/// authored, validated at publish and hashed into `config_hash` — and never
/// read by anything that runs, because the SQL the engine executed came from a
/// model file on a disk the tenant cannot reach.
///
/// The result lands as a **draft**, like any other authored version: compiling
/// is not activating, so a config edit can never silently re-cluster a corpus.
/// The response carries the compiler's notes, which say what it decided that
/// the config did not state outright — most importantly that the parameters are
/// declared rather than trained.
async fn compile_model_version(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Admin) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return model_registry_unavailable();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let config = match org_active_config(&client, &org_row).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let compiled = match er_compile::compile(&config) {
        Ok(c) => c,
        // A compile failure is the config author's to fix, and the message names
        // the comparison and the token.
        Err(e) => return detail_err(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()),
    };
    let entity_type = if config.entity_type.is_person() {
        "person"
    } else {
        "account"
    };
    // The compiled model keeps the org's current stamp: it scores the same
    // corpus against the same frozen term-frequency snapshot, so changing the
    // label here would void the TF lookup.
    let label = org_row
        .env
        .get("ER_MODEL_VERSION")
        .and_then(Value::as_str)
        .unwrap_or("v0001")
        .to_string();
    let provenance = json!({
        "origin": "compiled",
        "from_config_version": org_row.active_config_version,
        "config_hash": config_hash(&config),
        "notes": compiled.notes,
        "by": principal.actor,
    });
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::create_model_version(
        &client,
        &org_row.id,
        pg::DEFAULT_MODEL_NAME,
        entity_type,
        &compiled.model_json,
        &label,
        org_row.env.get("ER_TF_SNAPSHOT_ID").and_then(Value::as_str),
        &provenance,
        &principal.actor,
    )
    .await
    {
        Ok(m) => {
            let _ = pg::audit(
                &client,
                &principal.actor,
                Some(&org),
                "model.version.compile",
                &json!({ "name": m.name, "version": m.version, "model_hash": m.model_hash }),
            )
            .await;
            let mut body = model_summary(&m);
            body["notes"] = json!(compiled.notes);
            (StatusCode::CREATED, Json(body)).into_response()
        }
        Err(e) => pg_err(e),
    }
}

/// Viewer+: the tenant's model versions (`GET /v1/orgs/{org}/models`), newest
/// version of each name first, plus which one is active.
async fn models_index(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return model_registry_unavailable();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    match pg::list_model_versions(&client, &org_row.id).await {
        Ok(rows) => {
            // `active` is a LIST, one entry per name. It used to be
            // `rows.iter().find(state == "active")` — a single row — which was
            // well defined only while activation was unique per org. Since it
            // became unique per (org, NAME), `find` returns whichever active row
            // the list happened to order first, so the field would name an
            // arbitrary member of an ensemble. Ordered by name for a stable
            // render; an ensemble's RUN order comes from the published config's
            // `models:` list, never from here.
            let mut active: Vec<&_> = rows.iter().filter(|m| m.state == "active").collect();
            active.sort_by(|a, b| a.name.cmp(&b.name));
            Json(json!({
                "items": rows.iter().map(model_summary).collect::<Vec<_>>(),
                "active": active
                    .iter()
                    .map(|m| json!({
                        "name": m.name,
                        "version": m.version,
                        "model_hash": m.model_hash,
                    }))
                    .collect::<Vec<_>>(),
            }))
            .into_response()
        }
        Err(e) => pg_err(e),
    }
}

/// The 501 for model-registry routes when no Postgres backend is configured.
fn model_registry_unavailable() -> axum::response::Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({ "detail": "model registry requires Postgres" })),
    )
        .into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelVersionIn {
    /// The settings document. Validated by loading it through the engine's own
    /// parser, so an unusable model cannot be stored.
    model_json: String,
    /// The identity stamped onto rows this model scores. Required, not derived:
    /// it must agree with the term-frequency snapshot's `model_version`, and a
    /// guess that disagrees changes scores silently instead of failing.
    model_version_label: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    tf_snapshot_id: Option<String>,
    #[serde(default)]
    provenance: Option<Value>,
}

/// Admin+: mint a new model version (`POST /v1/orgs/{org}/models/versions`).
///
/// Versions are immutable — this is the only way a model changes, and it always
/// lands as a `draft`. Activation is a separate, explicit step so that uploading
/// a model can never silently re-cluster a tenant's corpus.
async fn create_model_version(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<ModelVersionIn>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Admin) {
        return e.into_response();
    }
    // Validate by loading it the way the engine will. A document that cannot be
    // parsed here would fail inside a dispatcher job later, where the only
    // symptom is a failed run.
    // `Settings::from_json_str` is exactly what the engine calls at run time.
    if let Err(e) = dedupe_core::Settings::from_json_str(&body.model_json) {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("model_json is not a loadable model: {}", errfmt::chain(&e)),
        );
    }
    let Some(dsn) = &state.dsn else {
        return model_registry_unavailable();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    // The entity type comes from the org's active config, not the caller: a
    // model scores against one canonical schema and the lake has exactly one.
    let entity_type = match org_active_config(&client, &org_row).await {
        Ok(c) => {
            if c.entity_type.is_person() {
                "person"
            } else {
                "account"
            }
        }
        Err(r) => return *r,
    };
    let name = body
        .name
        .as_deref()
        .unwrap_or(pg::DEFAULT_MODEL_NAME)
        .to_string();
    let provenance = body
        .provenance
        .unwrap_or_else(|| json!({ "origin": "uploaded", "by": principal.actor }));
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::create_model_version(
        &client,
        &org_row.id,
        &name,
        entity_type,
        &body.model_json,
        &body.model_version_label,
        body.tf_snapshot_id.as_deref(),
        &provenance,
        &principal.actor,
    )
    .await
    {
        Ok(m) => {
            let _ = pg::audit(
                &client,
                &principal.actor,
                Some(&org),
                "model.version.create",
                &json!({ "name": m.name, "version": m.version, "model_hash": m.model_hash }),
            )
            .await;
            (StatusCode::CREATED, Json(model_summary(&m))).into_response()
        }
        Err(e) => pg_err(e),
    }
}

/// Viewer+: one model version with its document and weights
/// (`GET /v1/orgs/{org}/models/{name}/versions/{version}`).
async fn get_model_version(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, name, version)): Path<(String, String, i64)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return model_registry_unavailable();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    match pg::model_version(&client, &org_row.id, &name, version).await {
        Ok(Some(m)) => {
            let mut body = model_summary(&m);
            body["model_json"] = json!(m.model_json);
            body["weights"] = model_weights(&m.model_json).unwrap_or(Value::Null);
            Json(body).into_response()
        }
        Ok(None) => detail_err(
            StatusCode::NOT_FOUND,
            format!("no model version {name}/{version}"),
        ),
        Err(e) => pg_err(e),
    }
}

/// Precision, recall, F1 and accuracy from a confusion matrix, or `Null` for any
/// ratio whose denominator is zero.
///
/// Null rather than `0.0`, which is O7a's rule and matters more here than on a
/// dashboard tile: a precision of 0.0 says "the model got every positive wrong",
/// while a null says "it made no positive predictions on this label set". Those
/// are opposite conclusions and a steward acts differently on each.
fn confusion_metrics(tp: u64, fp: u64, fn_: u64, tn: u64) -> Value {
    let ratio = |num: u64, den: u64| -> Value {
        if den == 0 {
            Value::Null
        } else {
            json!(num as f64 / den as f64)
        }
    };
    let precision = ratio(tp, tp + fp);
    let recall = ratio(tp, tp + fn_);
    // F1 needs both halves; it is null when either is, rather than 0.
    let f1 = match (precision.as_f64(), recall.as_f64()) {
        (Some(p), Some(r)) if p + r > 0.0 => json!(2.0 * p * r / (p + r)),
        _ => Value::Null,
    };
    json!({
        "tp": tp, "fp": fp, "fn": fn_, "tn": tn,
        "precision": precision,
        "recall": recall,
        "f1": f1,
        "accuracy": ratio(tp + tn, tp + fp + fn_ + tn),
    })
}

/// The confusion matrix over `labels` at `threshold`, as `(tp, fp, fn, tn)`.
fn confusion_at(
    labels: &[&er_entities::pair_label::PairLabel],
    threshold: f64,
) -> (u64, u64, u64, u64) {
    let mut counts = (0u64, 0u64, 0u64, 0u64);
    for l in labels {
        // Only evaluable rows reach here, so the score is present.
        let predicted_match = l.score_at_label.unwrap_or(f64::NEG_INFINITY) >= threshold;
        match (l.verdict.as_str(), predicted_match) {
            ("match", true) => counts.0 += 1,
            ("no_match", true) => counts.1 += 1,
            ("match", false) => counts.2 += 1,
            ("no_match", false) => counts.3 += 1,
            _ => {}
        }
    }
    counts
}

/// How one model version scored against the labels collected under it.
///
/// **The version is identified by its `model_version_label`, not its integer
/// version**, because that is what the scoring pass stamps onto
/// `match_scores.model_version` and what the labelling queue hands the client to
/// echo back. Filtering on the integer would match nothing and report an honest
/// but useless "0 labels".
///
/// Only labels carrying the score *that version* produced are evaluable
/// (`model_version` matches and `score_at_label` is present) — which is the
/// whole reason `score_at_label` is a column: its own description says it exists
/// "so a later accuracy pass can tell what the model thought at the time rather
/// than what it thinks now". A label recorded with no score in front of the
/// labeller (every `reviewer`-origin label, by construction) carries no
/// prediction and cannot be scored against any threshold.
///
/// Evaluating version B against labels collected under version A is deliberately
/// **not** attempted. It would mean re-scoring those pairs with B, which needs
/// the lake and the model bytes — that belongs with a training/evaluation job,
/// not a read endpoint, and silently mixing A's scores into B's metrics would be
/// worse than declining.
fn label_evaluation(
    labels: &[er_entities::pair_label::PairLabel],
    model_version_label: &str,
    threshold: f64,
) -> Value {
    let for_version: Vec<&er_entities::pair_label::PairLabel> = labels
        .iter()
        .filter(|l| l.model_version.as_deref() == Some(model_version_label))
        .collect();
    let is_verdict =
        |l: &er_entities::pair_label::PairLabel| l.verdict == "match" || l.verdict == "no_match";
    let evaluable: Vec<&er_entities::pair_label::PairLabel> = for_version
        .iter()
        .copied()
        .filter(|l| l.score_at_label.is_some() && is_verdict(l))
        .collect();

    // Reported beside the metrics, never silently dropped: a figure computed
    // over three labels has to be distinguishable from one over three hundred.
    let excluded = json!({
        "labels_for_version": for_version.len(),
        "evaluable": evaluable.len(),
        "no_score": for_version.iter().filter(|l| l.score_at_label.is_none()).count(),
        "not_a_verdict": for_version.iter().filter(|l| !is_verdict(l)).count(),
    });

    let (tp, fp, fn_, tn) = confusion_at(&evaluable, threshold);
    let mut by_origin = serde_json::Map::new();
    for origin in ["human", "reviewer", "llm"] {
        let subset: Vec<&er_entities::pair_label::PairLabel> = evaluable
            .iter()
            .copied()
            .filter(|l| l.origin == origin)
            .collect();
        if subset.is_empty() {
            continue;
        }
        let (tp, fp, misses, tn) = confusion_at(&subset, threshold);
        by_origin.insert(origin.to_string(), confusion_metrics(tp, fp, misses, tn));
    }

    // The trade-off curve, computed exactly at each step rather than from a
    // histogram: every label is already in memory, so there is no reason to
    // approximate at bucket edges.
    let curve: Vec<Value> = (0..=20)
        .map(|i| {
            let cut = f64::from(i) / 20.0;
            let (tp, fp, misses, tn) = confusion_at(&evaluable, cut);
            let mut point = confusion_metrics(tp, fp, misses, tn);
            point["threshold"] = json!(cut);
            point
        })
        .collect();

    json!({
        "model_version_label": model_version_label,
        "threshold": threshold,
        "counts": excluded,
        "overall": confusion_metrics(tp, fp, fn_, tn),
        "by_origin": Value::Object(by_origin),
        "curve": curve,
    })
}

/// Viewer+: how a model version scored against its own labels
/// (`GET /v1/orgs/{org}/models/{name}/versions/{version}/evaluation`).
///
/// Answers "can I trust this version" with the tenant's own verdicts rather than
/// a number from training. `?threshold=` overrides the org's configured
/// auto-merge threshold, which is the default because that is the cut the product
/// actually acts on.
async fn model_version_evaluation(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, name, version)): Path<(String, String, i64)>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "evaluation requires Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let model = match pg::model_version(&client, &org_row.id, &name, version).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            return detail_err(
                StatusCode::NOT_FOUND,
                format!("no model version {name}/{version}"),
            );
        }
        Err(e) => return pg_err(e),
    };
    let threshold = match read::parse_f64(params.get("threshold").map(String::as_str)) {
        Some(t) if (0.0..=1.0).contains(&t) => t,
        Some(_) => {
            return detail_err(
                StatusCode::UNPROCESSABLE_ENTITY,
                "threshold must be between 0 and 1",
            );
        }
        None => match org_active_config(&client, &org_row).await {
            Ok(c) => c.thresholds.auto_merge,
            Err(r) => return *r,
        },
    };
    let labels = match er_entities::pair_label::read::all(&client, &org_row.id).await {
        Ok(l) => l,
        Err(e) => return pg_err(e),
    };
    let mut body = label_evaluation(&labels, &model.model_version_label, threshold);
    body["name"] = json!(name);
    body["version"] = json!(version);
    body["state"] = json!(model.state);
    Json(body).into_response()
}

/// Admin+: activate a model version
/// (`POST /v1/orgs/{org}/models/{name}/versions/{version}/activate`).
///
/// Activation is explicit and recorded, because it decides what the next run
/// scores against — the thing DataGroomr avoids by pinning old models rather
/// than letting a model change re-cluster silently.
async fn activate_model_version(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, name, version)): Path<(String, String, i64)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Admin) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return model_registry_unavailable();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let target = match pg::model_version(&client, &org_row.id, &name, version).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            return detail_err(
                StatusCode::NOT_FOUND,
                format!("no model version {name}/{version}"),
            );
        }
        Err(e) => return pg_err(e),
    };
    // A model scores one canonical schema; activating the other entity type's
    // model against this lake would produce SQL referring to columns that do
    // not exist, discovered inside a run.
    let configured = match org_active_config(&client, &org_row).await {
        Ok(c) => {
            if c.entity_type.is_person() {
                "person"
            } else {
                "account"
            }
        }
        Err(r) => return *r,
    };
    if target.entity_type != configured {
        return detail_err(
            StatusCode::CONFLICT,
            format!(
                "model {name}/{version} is a {} model but this org resolves {configured} entities",
                target.entity_type
            ),
        );
    }
    let _ = pg::set_actor(&client, &principal.actor).await;
    if let Err(e) = pg::activate_model_version(&client, &org_row.id, &target.id).await {
        return pg_err(e);
    }
    let _ = pg::audit(
        &client,
        &principal.actor,
        Some(&org),
        "model.version.activate",
        &json!({ "name": name, "version": version, "model_hash": target.model_hash }),
    )
    .await;
    Json(json!({
        "name": name,
        "version": version,
        "state": "active",
        "model_hash": target.model_hash,
    }))
    .into_response()
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
    if existing.is_none()
        && let Err(e) = pg::merge_org_env(&client, &org, &json!({ "ER_LEXICON_PATH": path })).await
    {
        return pg_err(e);
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
        Some(&principal.actor),
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
// `constraints` sits with `blocking` because it changes the same thing: which
// pairs are candidates. Editing it needs a re-score, not a retrain.
//
// `models` joins them for the same reason, one level up: changing WHICH models
// score changes every pair's verdict set, and the members are already trained —
// so a re-score, not a retrain. (A member added to the list has never scored
// this corpus at all, which is exactly what tier B's full re-score fixes.)
const TIER_B_BLOCKS: [&str; 3] = ["blocking", "constraints", "models"];
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

    // The ensemble's members must exist as ACTIVE registry versions. Refused
    // here, as a 422 naming the member, rather than at run time: a publish is a
    // deliberate act with a human waiting on the response, while the same
    // mistake discovered in the dispatcher is a failed job with the tenant's
    // next rebuild already queued behind it.
    if !new_config.models.is_empty() {
        match pg::active_models(&client, &org_row.id).await {
            Ok(active) => {
                let have: std::collections::BTreeSet<&str> =
                    active.iter().map(|m| m.name.as_str()).collect();
                let missing: Vec<&str> = new_config
                    .models
                    .iter()
                    .map(|s| s.name.as_str())
                    .filter(|n| !have.contains(n))
                    .collect();
                if !missing.is_empty() {
                    let mut known: Vec<&str> = have.into_iter().collect();
                    known.sort_unstable();
                    return detail_err(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        format!(
                            "`models:` names {} with no ACTIVE version in the registry \
                             (active: {})",
                            missing.join(", "),
                            if known.is_empty() {
                                "none".to_string()
                            } else {
                                known.join(", ")
                            }
                        ),
                    );
                }
            }
            Err(e) => return pg_err(e),
        }
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
    // The published document's `correction_pass.cadence` becomes a real
    // schedule. It had been stored by every tenant config and read by nothing,
    // so the periodic correction pass it describes never ran.
    if let Ok(config) = load_config_str(&detail.yaml)
        && let Err(e) =
            pg::ensure_correction_schedule(&client, &org_row.id, &config.correction_pass.cadence)
                .await
    {
        return pg_err(e);
    }

    let mut jobs_enqueued = Vec::new();
    let rebuild_key = format!("publish:{version}:rebuild");
    let rebuild_params = json!({ "skip_ingest": true });
    match tier {
        Some("C") => {
            let train_key = format!("publish:{version}:train");
            match pg::enqueue(
                &client,
                &org_row.id,
                "train",
                0,
                Some(&train_key),
                None,
                Some(&principal.actor),
            )
            .await
            {
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
                Some(&principal.actor),
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
            Some(&principal.actor),
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
    // `config.published` (O3c). Queued through the outbox, like every other
    // event: a POST inline here would make a tenant's unreachable endpoint
    // slow down — or fail — their own publish.
    //
    // Best-effort: the version IS published and its rebuild jobs are queued, so
    // failing the response because an event could not be enqueued would report
    // failure for work that succeeded.
    if let Err(e) = webhook::emit(
        &client,
        &org_row.id,
        &org,
        "config.published",
        &json!({
            "version": version,
            "tier": tier,
            "changed_blocks": changed,
            "jobs_enqueued": jobs_enqueued,
            "published_by": principal.actor,
        }),
    )
    .await
    {
        eprintln!("config.published for {org} v{version} not queued: {e}");
    }
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
    /// Restrict the run to a `segments` entry of the active config (#54). A
    /// NAME, not a predicate: the SQL lives in the published config, which has
    /// been parsed and column-checked, so a request can never introduce any.
    #[serde(default)]
    segment: Option<String>,
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
        if let Err(e) = refuse_unless_active(&org_row, "jobs") {
            return *e;
        }
        // Resolve the segment NAME against the published config here, so an
        // unknown name is a 422 at submit time naming the segments that do
        // exist — rather than a job that sits in the queue and then fails.
        let mut params = None;
        if let Some(name) = &body.segment {
            let config = match org_active_config(&client, &org_row).await {
                Ok(c) => c,
                Err(r) => return *r,
            };
            if !config.segments.contains_key(name) {
                let known: Vec<&str> = config.segments.keys().map(String::as_str).collect();
                return detail_err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!(
                        "segment '{name}' is not in the published config (declared: {})",
                        if known.is_empty() {
                            "none".to_string()
                        } else {
                            known.join(", ")
                        }
                    ),
                )
                .into_response();
            }
            params = Some(json!({ "segment": name }));
        }
        return match pg::enqueue(
            &client,
            &org_row.id,
            &body.kind,
            body.priority,
            None,
            params.as_ref(),
            Some(&principal.actor),
        )
        .await
        {
            Ok(job) => {
                let _ = pg::audit(
                    &client,
                    &principal.actor,
                    Some(&org),
                    "job.submit",
                    &json!({ "job_id": job.id, "kind": body.kind, "segment": body.segment }),
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
        error_class: None,
        error_detail: None,
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
    // Record the job's outcome event, then — when given a client — drain it.
    // The real dispatcher splits these across its loop; doing both here keeps
    // this helper's contract "dispatch, and deliver what that produced", which
    // is what its tests observe. Best-effort: a subscriber lookup or delivery
    // failure never fails the job, whose outcome is already persisted.
    let _ = webhook::emit_job_event(client, &completed).await;
    if let Some(http) = webhook_http {
        let _ = webhook::drain(client, http, 16).await;
    }
    Ok(Some(completed))
}

/// List an org's jobs, newest first. Requires viewer+ in the org.
async fn list_jobs(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> axum::response::Response {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    // `limit` and `state` were accepted and IGNORED: this handler had no `Query`
    // extractor at all, and `pg::list_jobs` passed `i64::MAX` ("no page cap on
    // the per-org view"). Meanwhile the BFF forwards both (`PASSTHROUGH` in
    // `api/orgs/[org]/jobs/route.ts`), `useJobs` asks for `?limit=20`, and
    // `grow-your-data.spec.ts` asks for `?limit=3` and then asserts exactly
    // three kinds — which cannot hold on a tenant that accumulates jobs.
    //
    // Validated and parsed the same way `jobs_global` does, so the two job views
    // answer the same query the same way.
    if let Some(s) = params.get("state")
        && !JOB_STATES.contains(&s.as_str())
    {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("unknown state '{s}'"),
        );
    }
    let limit = read::parse_limit(params.get("limit").map(String::as_str));
    let state_filter = params.get("state").map(String::as_str);
    if let Some(dsn) = &state.dsn {
        let client = match connect_pg(dsn).await {
            Ok(c) => c,
            Err(r) => return *r,
        };
        let org_row = match org_or_404(&client, &org).await {
            Ok(r) => r,
            Err(r) => return *r,
        };
        // Through `list_jobs_all` scoped to this org: it already takes limit and
        // state and clamps the limit, so the per-org view no longer serializes
        // every job an org has ever had.
        return match pg::list_jobs_all(&client, Some(&org_row.id), state_filter, limit).await {
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
        .filter(|j| state_filter.is_none_or(|s| j.state == s))
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
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
                Ok(()) => {
                    let canceled = Job {
                        state: "canceled".to_string(),
                        ..job
                    };
                    // Record the event, so a subscriber learns the job ended.
                    // Cancel was the one terminal state that delivered nothing
                    // — a downstream system waiting on an outcome waited
                    // forever. Only RECORDED here: the dispatcher's drain does
                    // the sending, so a slow or dead subscriber cannot hold up
                    // the response to a cancel that is already persisted.
                    let _ = webhook::emit_job_event(&client, &canceled).await;
                    (StatusCode::OK, Json(json!(canceled))).into_response()
                }
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
pub(crate) fn entity_type_from_env(env: &Value) -> er_config::EntityType {
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
    if let Some(cursor) = &page.next_cursor
        && let Ok(v) = axum::http::HeaderValue::from_str(cursor)
    {
        headers.insert("x-er-next-cursor", v);
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
    // The clusters an over-merge hides in are the LARGE ones, and a page
    // ordered by entity id will not surface them on its own.
    let min_members = params
        .get("min_members")
        .and_then(|v| v.trim().parse::<i64>().ok());
    match read::duplicates(
        &session,
        entity_type,
        snapshot,
        after.as_deref(),
        min_members,
        limit,
    ) {
        Ok((items, next_cursor)) => Json(json!({
            "items": items,
            "snapshot": snapshot,
            "next_cursor": next_cursor,
        }))
        .into_response(),
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
}

/// Viewer+: one run's per-source duplicate attribution
/// (`GET /v1/orgs/{org}/insights/sources`). Query `run_id`; defaults to the
/// newest snapshot's run.
///
/// The corpus-wide rate says a tenant has a problem; this says which feed it
/// came from, which is the difference between knowing and being able to act.
async fn insight_sources(
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
    let snapshot = match read::latest_snapshot(&session) {
        Ok(s) => s,
        Err(e) => return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}")),
    };
    // Default to the newest run rather than making the caller know one:
    // "which source is causing this" is a question asked about now.
    let run_id = match params.get("run_id").map(String::as_str) {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => match read::insights(&session, snapshot, 1) {
            Ok(series) => match series.first().and_then(|r| r["run_id"].as_str()) {
                Some(id) => id.to_string(),
                None => {
                    return Json(json!({ "snapshot": snapshot, "run_id": null, "sources": [] }))
                        .into_response();
                }
            },
            Err(e) => {
                return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}"));
            }
        },
    };
    match read::insight_sources(&session, snapshot, &run_id) {
        Ok(sources) => Json(json!({
            "snapshot": snapshot,
            "run_id": run_id,
            "sources": sources,
        }))
        .into_response(),
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
}

/// Viewer+: the resolution metrics over time (`GET /v1/orgs/{org}/insights`).
///
/// Every dashboard number has always been computed live, so it could say what
/// is true now and nothing about the direction of travel — and "is the
/// duplicate rate going up" is the question an operator actually has. Nothing
/// was retained to answer it until runs started leaving a snapshot behind.
///
/// Newest first, `?limit=` to widen the window.
async fn insights(
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
    let snapshot = match read::page_start(&session, None) {
        Ok((snap, _)) => snap,
        Err(read::PageStartError::BadCursor) => {
            return detail_err(StatusCode::UNPROCESSABLE_ENTITY, "invalid cursor");
        }
        Err(read::PageStartError::Lake(e)) => {
            return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}"));
        }
    };
    let limit = read::parse_limit(params.get("limit").map(String::as_str));
    match read::insights(&session, snapshot, limit) {
        Ok(series) => Json(json!({ "snapshot": snapshot, "series": series })).into_response(),
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
}

/// Viewer+: the corpus's data-quality picture (`GET /v1/orgs/{org}/quality`).
///
/// The distribution in buckets, the per-dimension means, and the worst records
/// as a work queue. `?limit=` sizes the queue.
///
/// A tenant with no `data_quality` block reads as `configured: false` rather
/// than erroring — scoring is opt-in, and asking about it is a fair question
/// whether or not it is switched on.
async fn quality(
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
    let snapshot = match read::page_start(&session, None) {
        Ok((snap, _)) => snap,
        Err(read::PageStartError::BadCursor) => {
            return detail_err(StatusCode::UNPROCESSABLE_ENTITY, "invalid cursor");
        }
        Err(read::PageStartError::Lake(e)) => {
            return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}"));
        }
    };
    let limit = read::parse_limit(params.get("limit").map(String::as_str));
    match read::quality(&session, snapshot, limit) {
        Ok(body) => Json(body).into_response(),
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
}

/// Viewer+: active pairwise match scores (`GET /v1/orgs/{org}/match-scores`).
/// Keyset page; query `cursor`, `limit`, `band_low`/`band_high` (probability
/// band). The band names must match what the BFF forwards
/// (`src/app/api/orgs/[org]/match-scores/route.ts`): a rename on one side only
/// drops the filter silently, which is what `min`/`max` here used to do.
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
    let band_low = read::parse_f64(params.get("band_low").map(String::as_str));
    let band_high = read::parse_f64(params.get("band_high").map(String::as_str));
    match read::match_scores_list(
        &session,
        snapshot,
        after.as_deref(),
        band_low,
        band_high,
        limit,
    ) {
        Ok((items, next_cursor)) => Json(json!({
            "items": items,
            "snapshot": snapshot,
            "next_cursor": next_cursor,
        }))
        .into_response(),
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
}

/// Viewer+: what to label next (`GET /v1/orgs/{org}/labeling/next`).
///
/// Serves the pairs the model is LEAST sure about — nearest the decision
/// threshold first — with a coverage summary of the labels already recorded.
/// Pairs already labelled are excluded, so a session makes progress instead of
/// re-offering the same ten pairs.
///
/// The engine has always been able to learn from labels (`m_from_labels`,
/// `truth_space_from_labels_*` in dedupe-core) and there was no way to produce
/// any: the whole accuracy module had no caller outside dedupe-cli and its own
/// tests. This endpoint and `POST .../labels` are the missing input.
///
/// Coverage is reported rather than just a count because the SPLIT is what
/// decides whether the labels are worth training on: a set that is 95%
/// `no_match` teaches a model almost nothing about matches, and a single total
/// hides that completely.
async fn labeling_next(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "labeling requires Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let config = match org_active_config(&client, &org_row).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let labels = match er_entities::pair_label::read::all(&client, &org_row.id).await {
        Ok(l) => l,
        Err(e) => return pg_err(e),
    };
    let coverage = label_coverage(&labels);
    let labelled: std::collections::BTreeSet<(String, String)> = labels
        .iter()
        .map(|l| (l.rec_a_key.clone(), l.rec_b_key.clone()))
        .collect();

    let session = match org_lake(&state, &org).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let snapshot = match read::page_start(&session, None) {
        Ok((snap, _)) => snap,
        Err(read::PageStartError::BadCursor) => {
            return detail_err(StatusCode::UNPROCESSABLE_ENTITY, "invalid cursor");
        }
        Err(read::PageStartError::Lake(e)) => {
            return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}"));
        }
    };
    // The gray band is where a label is worth having: above `auto_merge` the
    // engine merges without asking and below `review_low` it never scored the
    // pair at all, so neither end teaches it anything. A caller may narrow
    // further but not widen past what was scored.
    let threshold = config.thresholds.auto_merge;
    let band_low = read::parse_f64(params.get("min").map(String::as_str))
        .unwrap_or(config.thresholds.review_low)
        .max(config.thresholds.review_low);
    let band_high = read::parse_f64(params.get("max").map(String::as_str)).unwrap_or(1.0);
    let limit = read::parse_limit(params.get("limit").map(String::as_str));
    match read::uncertain_pairs(
        &session,
        snapshot,
        threshold,
        (band_low, band_high),
        &labelled,
        limit,
    ) {
        Ok(items) => Json(json!({
            "items": items,
            "snapshot": snapshot,
            "threshold": threshold,
            "band": { "min": band_low, "max": band_high },
            "coverage": coverage,
        }))
        .into_response(),
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
}

/// How much labelled signal a tenant has, split by verdict and origin.
///
/// Counted in Rust rather than aggregated in SQL because a control-plane read
/// declares `returns: entity`, and the training pass needs the rows whole
/// anyway.
fn label_coverage(labels: &[er_entities::pair_label::PairLabel]) -> serde_json::Value {
    let count = |f: &dyn Fn(&er_entities::pair_label::PairLabel) -> bool| {
        labels.iter().filter(|l| f(l)).count()
    };
    json!({
        "total": labels.len(),
        "matches": count(&|l| l.verdict == "match"),
        "non_matches": count(&|l| l.verdict == "no_match"),
        "by_origin": {
            "human": count(&|l| l.origin == "human"),
            "reviewer": count(&|l| l.origin == "reviewer"),
            "llm": count(&|l| l.origin == "llm"),
        },
    })
}

#[derive(Deserialize)]
struct LabelIn {
    rec_a_key: String,
    rec_b_key: String,
    verdict: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    model_version: Option<String>,
    #[serde(default)]
    score_at_label: Option<f64>,
}

/// Steward+: record a verdict on a pair (`POST /v1/orgs/{org}/labels`).
///
/// Upserts on the pair, so re-labelling corrects rather than stacks — two
/// contradictory rows for one pair would make the training set ambiguous with
/// no principled way to choose between them.
///
/// The endpoint canonicalises the pair (lower key first) before writing, because
/// `match_scores` stores pairs that way and a label recorded the other way round
/// would silently describe a pair nothing ever scored.
async fn create_label(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<LabelIn>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    if !["match", "no_match"].contains(&body.verdict.as_str()) {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "verdict must be 'match' or 'no_match', not '{}' — a labeller who is unsure                  should skip, because a guessed label is indistinguishable afterwards from a                  confident one",
                body.verdict
            ),
        );
    }
    let (a, b) = canonical_pair(&body.rec_a_key, &body.rec_b_key);
    if a.is_empty() || b.is_empty() {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "rec_a_key and rec_b_key are both required",
        );
    }
    if a == b {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "a record cannot be labelled against itself",
        );
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "labeling requires Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    match record_label(
        &client,
        &org_row.id,
        (&a, &b),
        &body.verdict,
        "human",
        &principal.actor,
        body.model_version.as_deref(),
        body.score_at_label,
        body.note.as_deref(),
    )
    .await
    {
        Ok(label) => {
            let _ = pg::audit(
                &client,
                &principal.actor,
                Some(&org),
                "label.record",
                &json!({ "pair": [a, b], "verdict": body.verdict }),
            )
            .await;
            (StatusCode::CREATED, Json(json!(label))).into_response()
        }
        Err(e) => pg_err(e),
    }
}

/// Viewer+: the labels a tenant has recorded (`GET /v1/orgs/{org}/labels`),
/// newest first, filterable by `verdict` and `origin`.
async fn labels_list(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "labeling requires Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let limit = read::parse_limit(params.get("limit").map(String::as_str));
    match er_entities::pair_label::read::page(
        &client,
        &org_row.id,
        params.get("verdict").map(String::as_str),
        params.get("origin").map(String::as_str),
        limit,
    )
    .await
    {
        Ok(rows) => Json(json!({ "labels": rows })).into_response(),
        Err(e) => pg_err(e),
    }
}

/// The pair, in the canonical order `match_scores` stores it.
fn canonical_pair(a: &str, b: &str) -> (String, String) {
    let (a, b) = (a.trim(), b.trim());
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// Record one label, upserting on the pair.
///
/// Shared by the explicit labelling endpoint and the implicit feedback a review
/// resolution produces, so both land in one table with their `origin` recorded
/// — which is what lets training weight or exclude a source later.
#[expect(
    clippy::too_many_arguments,
    reason = "every one is a column of the row being written; a struct here would               be indirection for its own sake"
)]
async fn record_label(
    client: &tokio_postgres::Client,
    org_id: &str,
    pair: (&str, &str),
    verdict: &str,
    origin: &str,
    actor: &str,
    model_version: Option<&str>,
    score_at_label: Option<f64>,
    note: Option<&str>,
) -> Result<er_entities::pair_label::PairLabel, tokio_postgres::Error> {
    let rows = er_entities::pair_label::create::record(
        client,
        &er_ids::new_v7(),
        org_id,
        pair.0,
        pair.1,
        verdict,
        origin,
        actor,
        model_version,
        score_at_label,
        note,
    )
    .await?;
    Ok(rows
        .into_iter()
        .next()
        .expect("the upsert always returns its row"))
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveIn {
    /// Which declared source's column names the record uses. Required, because
    /// the payload is mapped through that source's declared columns — the same
    /// mapping an ingested file goes through.
    source: String,
    /// The record itself, in that source's raw column names.
    record: Value,
    #[serde(default)]
    limit: Option<usize>,
}

/// Viewer+: resolve an UNSAVED record against the corpus
/// (`POST /v1/orgs/{org}/resolve`).
///
/// The online half of the product: a form, an ingestion pipeline or a
/// middleware layer posts a record and learns which entity it belongs to, and
/// how confident that is, **before** saving it. This is how the engine embeds
/// in someone else's system rather than owning their data, and it is table
/// stakes for offering resolution as a service.
///
/// Viewer-level and read-only: asking who a record is writes nothing, the same
/// reasoning that makes the pre-merge report and the blocking estimate
/// viewer-accessible. A caller that wants the record *stored* sends it to the
/// import path afterwards — resolving and ingesting are deliberately separate,
/// so a lookup can never mutate the corpus it is asking about.
///
/// The payload goes through the same standardization and the same model the
/// batch path uses, so an online decision and a batch one agree. The response
/// carries the per-comparison evidence, so a caller can explain the decision it
/// just made.
async fn resolve_record(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<ResolveIn>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    if !body.record.is_object() {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "record must be a JSON object of the source's columns",
        );
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "resolve requires Postgres");
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
                format!("org '{org}' is {}; resolve needs an active org", o.state),
            );
        }
        Ok(None) => return detail_err(StatusCode::NOT_FOUND, format!("no org '{org}'")),
        Err(e) => return pg_err(e),
    };
    let limit = body.limit.unwrap_or(10).clamp(1, 100);
    let source = body.source.clone();
    let record = body.record.clone();

    match tokio::task::spawn_blocking(move || resolve_blocking(&env, &source, &record, limit)).await
    {
        Ok(Ok(body)) => Json(body).into_response(),
        Ok(Err(msg)) => detail_err(StatusCode::CONFLICT, msg),
        Err(e) => detail_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("resolve task panicked: {e}"),
        ),
    }
}

/// The blocking half of [`resolve_record`].
fn resolve_blocking(
    env: &Value,
    source: &str,
    record: &Value,
    limit: usize,
) -> Result<Value, String> {
    let s = |k: &str| env.get(k).and_then(Value::as_str).filter(|v| !v.is_empty());
    let catalog = s("ER_LAKE_CATALOG").ok_or("org env missing ER_LAKE_CATALOG")?;
    let data_path = s("ER_LAKE_DATA_PATH").ok_or("org env missing ER_LAKE_DATA_PATH")?;
    let model_path =
        s("ER_MODEL_PATH").ok_or("no model yet (ER_MODEL_PATH); run a full run first")?;
    let tf_path = s("ER_TF_PATH").ok_or("no model yet (ER_TF_PATH); run a full run first")?;
    let config_path = s("ER_CONFIG_PATH").ok_or("org env missing ER_CONFIG_PATH")?;

    let settings_json =
        std::fs::read_to_string(model_path).map_err(|e| format!("read model: {e}"))?;
    let config_text =
        std::fs::read_to_string(config_path).map_err(|e| format!("read config: {e}"))?;
    let config = er_config::load_config_str(&config_text)
        .map_err(|e| format!("config: {}", errfmt::chain(&e)))?;
    let lexicon = s("ER_LEXICON_PATH")
        .map(load_lexicon_file)
        .unwrap_or_default();
    let session = er_cli::open_lake(catalog, data_path).map_err(|e| format!("open lake: {e:#}"))?;
    let model = er_cli::MatchModel {
        settings_json: &settings_json,
        tf_csv_path: std::path::Path::new(tf_path),
        model_version: s("ER_MODEL_VERSION").unwrap_or("v0001"),
        tf_snapshot_id: s("ER_TF_SNAPSHOT_ID").unwrap_or("tf"),
    };
    let result = er_cli::resolve_record(&session, &config, &model, source, record, &lexicon, limit)
        .map_err(|e| format!("{e:#}"))?;
    serde_json::to_value(&result).map_err(|e| format!("rendering the result: {e}"))
}

/// Read a nickname lexicon CSV the way the dispatcher does. Resolve needs the
/// same lexicon the corpus was standardized with, or the probe record's name
/// variants would not line up with the corpus it is scored against.
fn load_lexicon_file(path: &str) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            l.split_once(',')
                .map(|(a, b)| (a.to_string(), b.to_string()))
        })
        .collect()
}

/// Viewer+: what a run would cost before it runs
/// (`GET /v1/orgs/{org}/blocking-estimate`). Query: `largest` (how many
/// pathological blocks to name, default 5).
///
/// The competitor's answer to a large corpus is a hard three-hour cap on
/// analysis, with the documented workaround "split your dataset with filters or
/// raise the confidence floor" — two guesses made blind. Blocking decides the
/// entire cost of a run, so the useful thing is the number up front: how many
/// pairs each rule generates, how many it adds that no earlier rule already
/// found, and which single key is large enough to dominate everything.
///
/// Read-only, and viewer-level for the same reason the pre-merge report is:
/// estimating a cost writes nothing.
async fn blocking_estimate(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "blocking estimate requires Postgres",
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
                format!(
                    "org '{org}' is {}; an estimate needs an active org",
                    o.state
                ),
            );
        }
        Ok(None) => return detail_err(StatusCode::NOT_FOUND, format!("no org '{org}'")),
        Err(e) => return pg_err(e),
    };
    let largest = params
        .get("largest")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(5)
        .min(50);

    match tokio::task::spawn_blocking(move || blocking_estimate_blocking(&env, largest)).await {
        Ok(Ok(body)) => Json(body).into_response(),
        Ok(Err(msg)) => detail_err(StatusCode::CONFLICT, msg),
        Err(e) => detail_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("estimate task panicked: {e}"),
        ),
    }
}

/// The blocking half of [`blocking_estimate`]: open the org's lake and model,
/// take the blocking rules from the model the run will actually use, and count.
///
/// The rules come from the model rather than the config because the model is
/// what executes — asking the config would estimate a run nobody is going to
/// perform.
fn blocking_estimate_blocking(env: &Value, largest: usize) -> Result<Value, String> {
    let s = |k: &str| env.get(k).and_then(Value::as_str).filter(|v| !v.is_empty());
    let catalog = s("ER_LAKE_CATALOG").ok_or("org env missing ER_LAKE_CATALOG")?;
    let data_path = s("ER_LAKE_DATA_PATH").ok_or("org env missing ER_LAKE_DATA_PATH")?;
    let model_path =
        s("ER_MODEL_PATH").ok_or("no model yet (ER_MODEL_PATH); publish or compile one first")?;
    let settings_json =
        std::fs::read_to_string(model_path).map_err(|e| format!("read model: {e}"))?;

    // The rules the model blocks on, in its own order — the marginal counts are
    // order-dependent, so reading them from anywhere else would mislead.
    let parsed: Value =
        serde_json::from_str(&settings_json).map_err(|e| format!("model is not JSON: {e}"))?;
    let rules: Vec<String> = parsed
        .get("blocking_rules_to_generate_predictions")
        .and_then(Value::as_array)
        .map(|rules| {
            rules
                .iter()
                .filter_map(|r| {
                    r.as_str().map(str::to_string).or_else(|| {
                        r.get("blocking_rule")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    if rules.is_empty() {
        return Err("the active model declares no blocking rules".to_string());
    }

    let session = er_cli::open_lake(catalog, data_path).map_err(|e| format!("open lake: {e:#}"))?;
    let estimate = er_cli::estimate_blocking(&session, &settings_json, &rules, largest)
        .map_err(|e| format!("{e:#}"))?;
    serde_json::to_value(&estimate).map_err(|e| format!("rendering the estimate: {e}"))
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
    // The confidence band (O6). A steward with a 40,000-item queue wants the
    // high-confidence tail first — it clears fastest — and the queue was FIFO
    // by id with no way to say so.
    let probability = |key: &str| params.get(key).and_then(|v| v.trim().parse::<f64>().ok());
    let text = |key: &str| {
        params
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.trim().is_empty())
    };
    let filters = read::ReviewFilters {
        status: text("status"),
        reason: text("reason"),
        min_probability: probability("min_probability"),
        max_probability: probability("max_probability"),
    };
    let band = (filters.min_probability, filters.max_probability);
    if let (Some(lo), Some(hi)) = band
        && lo > hi
    {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("min_probability {lo} is above max_probability {hi}; the band is empty"),
        );
    }
    let (items, next_cursor) =
        match read::reviews_list(&session, snapshot, after.as_deref(), filters, limit) {
            Ok(page) => page,
            Err(e) => {
                return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}"));
            }
        };
    // What the band HIDES, reported rather than left to be discovered. A
    // `never_unsatisfiable` escalation carries no probability (it is a
    // structural contradiction, not a scored pair), so it is in no band — and
    // it is the highest-priority item in the queue. Silently removing it behind
    // a slider is how it goes unanswered forever.
    let unscored_hidden = if band.0.is_some() || band.1.is_some() {
        read::unscored_open_reviews(&session, snapshot).unwrap_or(0)
    } else {
        0
    };
    Json(json!({
        "items": items,
        "snapshot": snapshot,
        "next_cursor": next_cursor,
        "unscored_hidden": unscored_hidden,
    }))
    .into_response()
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

/// The upload cap, mirrored client-side by the BFF so an oversize file fails
/// before it crosses the wire. Enforced here two ways: as the route's body limit
/// (so a lying `content-length` cannot get past the server) and as a running
/// count while the part streams to disk. Overridable per state via
/// [`AppState::with_max_import_bytes`].
pub const DEFAULT_MAX_IMPORT_BYTES: usize = 256 * 1024 * 1024;

/// An org's ACTIVE published config, parsed. Drafts do not count: a request is
/// only served against the config the running pipeline actually uses.
///
/// Takes the already-fetched [`pg::OrgDetail`] rather than an org name, because
/// `pg::get_config_version` keys on the org **id** while `pg::org_publish_context`
/// keys on the **name** — a footgun that reaching through the row avoids, and
/// that `org_source_ranks` tripped over before this existed.
async fn org_active_config(
    client: &tokio_postgres::Client,
    org_row: &pg::OrgDetail,
) -> Result<er_config::Config, Box<axum::response::Response>> {
    let Some(active) = org_row.active_config_version else {
        return Err(Box::new(detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no published config; publish a config version first",
        )));
    };
    let yaml = match pg::get_config_version(client, &org_row.id, active).await {
        Ok(Some(d)) => d.yaml,
        Ok(None) => {
            return Err(Box::new(detail_err(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!(
                    "config version {active} is missing for org '{}'",
                    org_row.name
                ),
            )));
        }
        Err(e) => return Err(Box::new(pg_err(e))),
    };
    load_config_str(&yaml).map_err(|e| Box::new(config_invalid(&e).into_response()))
}

/// The checks every delivery shares, whatever shape it arrived in: the org
/// exists and is active, the source is declared in the ACTIVE config, and the
/// org has a `drop_root` to land into (created if absent).
///
/// Shared by the file upload and the JSON push so the two cannot drift: a
/// delivery accepted one way and refused the other would be a surprise, and the
/// `source` value becomes a path segment in both.
async fn delivery_preconditions(
    client: &tokio_postgres::Client,
    org: &str,
    source: &str,
) -> Result<(pg::OrgDetail, er_config::Config, String), Box<axum::response::Response>> {
    let org_row = org_or_404(client, org).await?;
    if org_row.state != "active" {
        return Err(Box::new(detail_err(
            StatusCode::CONFLICT,
            format!(
                "org '{org}' is {}; imports are accepted only when active",
                org_row.state
            ),
        )));
    }
    let config = org_active_config(client, &org_row).await?;
    if !config.sources.contains_key(source) {
        let known: Vec<&str> = config.sources.keys().map(String::as_str).collect();
        return Err(Box::new(detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "source '{source}' is not in the active config (known: {}); \
                 publish a config version containing sources.{source} first",
                known.join(", ")
            ),
        )));
    }
    let Some(drop_root) = org_row.drop_root.clone() else {
        return Err(Box::new(detail_err(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("org '{org}' has no drop_root to land a delivery into"),
        )));
    };
    // The source is a key of the active config, so it cannot traverse; assert it
    // anyway, because this value becomes a path segment.
    if !source
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(Box::new(detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("source '{source}' is not a valid path segment"),
        )));
    }
    if let Err(e) = tokio::fs::create_dir_all(&drop_root).await {
        return Err(Box::new(detail_err(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("drop_root {drop_root} is not writable: {e}"),
        )));
    }
    Ok((org_row, config, drop_root))
}

/// Publish a staged delivery and enqueue the run that reads it.
///
/// The rename is the point, and it is why both delivery paths end here: the
/// dispatcher reads `<drop_root>/<source>.csv` directly, so a run claimed
/// mid-write must never see a half-written file. Until the rename succeeds there
/// is no delivery at all.
struct StagedDelivery<'a> {
    org: &'a str,
    org_id: &'a str,
    source: &'a str,
    delivery_id: &'a str,
    /// Where the bytes are now. Renamed into place by `finish_delivery`.
    temp_path: &'a str,
    bytes: u64,
    /// The audit action naming how it arrived (`import.upload` / `import.push`).
    action: &'a str,
}

async fn finish_delivery(
    client: &tokio_postgres::Client,
    actor: &str,
    d: &StagedDelivery<'_>,
) -> axum::response::Response {
    let StagedDelivery {
        org,
        org_id,
        source,
        delivery_id,
        temp_path,
        bytes,
        action,
    } = *d;
    let final_path = format!(
        "{}/{source}.csv",
        temp_path.rsplit_once('/').map_or("", |(d, _)| d)
    );
    if let Err(e) = tokio::fs::rename(temp_path, &final_path).await {
        let _ = tokio::fs::remove_file(temp_path).await;
        return detail_err(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("cannot publish the delivery: {e}"),
        );
    }
    // No idempotency key: two deliveries of the same source are two deliveries
    // and must run twice, so an upsert on a per-source key would be wrong.
    let job = match pg::enqueue(
        client,
        org_id,
        "run_all_incremental",
        0,
        None,
        Some(&json!({ "source": source, "delivery_id": delivery_id })),
        Some(actor),
    )
    .await
    {
        Ok(j) => j,
        Err(e) => return pg_err(e),
    };
    let _ = pg::audit(
        client,
        actor,
        Some(org),
        action,
        &json!({
            "source": source,
            "delivery_id": delivery_id,
            "bytes": bytes,
            "job_id": job.id,
        }),
    )
    .await;
    // `import.received` (O3c) — the delivery is committed to the drop root and
    // its run is queued. Deliberately NOT "the import finished": that is a
    // different question and `job.succeeded` already answers it, keyed by the
    // `job_id` this payload carries so a subscriber can correlate the two.
    //
    // Both delivery paths reach here (the CSV upload and the JSON push), which
    // is why the emit is in `finish_delivery` rather than in either endpoint —
    // the same reason E9a put the preconditions here.
    if let Err(e) = webhook::emit(
        client,
        org_id,
        org,
        "import.received",
        &json!({
            "source": source,
            "delivery_id": delivery_id,
            "bytes": bytes,
            "job_id": job.id,
        }),
    )
    .await
    {
        eprintln!("import.received for {org}/{source} not queued: {e}");
    }
    (
        StatusCode::ACCEPTED,
        Json(json!({ "delivery_id": delivery_id, "job": job })),
    )
        .into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PushRecordsIn {
    /// Which declared source these records belong to; their keys are that
    /// source's raw column names.
    source: String,
    /// The records. Each is an object of that source's raw columns — the same
    /// shape a row of its CSV would have.
    records: Vec<Value>,
}

/// Steward+: push records as JSON (`POST /v1/orgs/{org}/records:ingest`).
///
/// The write half of the batch API. Before this, data could only arrive as a
/// file: there was no way for a pipeline to hand records over programmatically,
/// which is the integration every data service is asked for first.
///
/// It deliberately takes the SAME path a file does — the records are rendered
/// into the source's declared CSV shape, staged into the drop root, and a run
/// is enqueued. Landing them into the lake directly from the API would mean the
/// API competing for the per-tenant writer lock the dispatcher holds, and a
/// second ingest path to keep honest against the first.
///
/// The CSV columns come from the source's DECLARED mapping rather than from the
/// records' own keys, so a record that omits a column lands as empty rather than
/// shifting every later column — and an unexpected key is reported instead of
/// being silently dropped.
async fn push_records(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<PushRecordsIn>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    if body.records.is_empty() {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "records must not be empty",
        );
    }
    if body.records.len() > MAX_PUSHED_RECORDS {
        return detail_err(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "{} records exceeds the {MAX_PUSHED_RECORDS}-record cap for one push; \
                 send a file for a bulk load",
                body.records.len()
            ),
        );
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "imports require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let (org_row, config, drop_root) =
        match delivery_preconditions(&client, &org, &body.source).await {
            Ok(t) => t,
            Err(r) => return *r,
        };
    let spec = &config.sources[&body.source];

    // The columns this source declares, in a stable order: its record id, its
    // updated-at stamp, then every mapped column. Taken from the config rather
    // than from the records so a missing value is an empty cell, not a shift.
    let mut columns: Vec<String> = vec![spec.record_id_column.clone()];
    if !spec.updated_at_column.is_empty() && spec.updated_at_column != spec.record_id_column {
        columns.push(spec.updated_at_column.clone());
    }
    for raw in spec.columns.values() {
        if !columns.contains(raw) {
            columns.push(raw.clone());
        }
    }

    let mut csv = String::new();
    csv.push_str(&columns.join(","));
    csv.push('\n');
    for (i, record) in body.records.iter().enumerate() {
        let Some(object) = record.as_object() else {
            return detail_err(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("record {i} is not a JSON object"),
            );
        };
        // An unexpected key is reported, not dropped: silently ignoring it would
        // mean a caller believing it sent data the pipeline never saw.
        if let Some(unknown) = object.keys().find(|k| !columns.contains(k)) {
            return detail_err(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!(
                    "record {i} has column {unknown:?}, which source '{}' does not declare \
                     (declared: {})",
                    body.source,
                    columns.join(", ")
                ),
            );
        }
        let row: Vec<String> = columns
            .iter()
            .map(|column| csv_cell(object.get(column)))
            .collect();
        csv.push_str(&row.join(","));
        csv.push('\n');
    }

    let delivery_id = er_ids::new_v7();
    let temp_path = format!("{drop_root}/.{}.{delivery_id}.csv.part", body.source);
    let bytes = csv.len() as u64;
    if let Err(e) = tokio::fs::write(&temp_path, csv.as_bytes()).await {
        return detail_err(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("cannot write into {drop_root}: {e}"),
        );
    }
    finish_delivery(
        &client,
        &principal.actor,
        &StagedDelivery {
            org: &org,
            org_id: &org_row.id,
            source: &body.source,
            delivery_id: &delivery_id,
            temp_path: &temp_path,
            bytes,
            action: "import.push",
        },
    )
    .await
}

/// Render one JSON value as a CSV cell: quoted when it has to be, empty for
/// null or a missing key. Numbers and booleans render as themselves so a caller
/// does not have to stringify everything.
fn csv_cell(value: Option<&Value>) -> String {
    let raw = match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    };
    if raw.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", raw.replace('"', "\"\""))
    } else {
        raw
    }
}

/// How many records one JSON push may carry. A bigger load belongs in a file:
/// this body is buffered and parsed in memory, while the upload path streams.
const MAX_PUSHED_RECORDS: usize = 10_000;

/// Steward+: upload one source's delivery
/// (`POST /v1/orgs/{org}/imports?source=<source>`), then enqueue the run that
/// picks it up.
///
/// This is the HTTP half of ingest. The dispatcher reads each configured
/// source's delivery from `<drop_root>/<source>.csv`, so an upload lands the file
/// there under a temp name and renames it into place — atomic, so a dispatcher
/// that claims the run mid-upload cannot read a half-written CSV.
///
/// The response mirrors what the BFF and its mock already consume:
/// `202 {"delivery_id": …, "job": {…}}`.
///
/// Status contract (the BFF maps each one): **202** accepted · **409** org not
/// active · **413** over the state's import cap · **422** missing `source`, no
/// `file` part, or a source absent from the active config · **503** the org has
/// no `drop_root` to land into.
async fn upload_import(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    mut multipart: axum::extract::Multipart,
) -> impl IntoResponse {
    use tokio::io::AsyncWriteExt;

    let cap = state.max_import_bytes;
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    let Some(source) = params
        .get("source")
        .map(String::as_str)
        .filter(|s| !s.is_empty())
    else {
        return detail_err(StatusCode::UNPROCESSABLE_ENTITY, "source is required");
    };
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "imports require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let (org_row, _config, drop_root) = match delivery_preconditions(&client, &org, source).await {
        Ok(t) => t,
        Err(r) => return *r,
    };
    let delivery_id = er_ids::new_v7();
    let temp_path = format!("{drop_root}/.{source}.{delivery_id}.csv.part");

    // Stream the `file` part to the temp path. Chunked rather than buffered: the
    // cap is 256 MiB and a handler must not hold that per concurrent upload.
    let mut wrote: Option<u64> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                // A body over the route's limit surfaces here, not as a clean EOF.
                let status = if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    StatusCode::PAYLOAD_TOO_LARGE
                } else {
                    StatusCode::UNPROCESSABLE_ENTITY
                };
                return detail_err(status, format!("malformed multipart body: {e}"));
            }
        };
        if field.name() != Some("file") {
            continue;
        }
        let mut file = match tokio::fs::File::create(&temp_path).await {
            Ok(f) => f,
            Err(e) => {
                return detail_err(
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!("cannot write into {drop_root}: {e}"),
                );
            }
        };
        let mut total: u64 = 0;
        let mut field = field;
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    total += chunk.len() as u64;
                    if total > cap as u64 {
                        let _ = tokio::fs::remove_file(&temp_path).await;
                        return detail_err(
                            StatusCode::PAYLOAD_TOO_LARGE,
                            format!("imports are capped at {cap} bytes"),
                        );
                    }
                    if let Err(e) = file.write_all(&chunk).await {
                        let _ = tokio::fs::remove_file(&temp_path).await;
                        return detail_err(
                            StatusCode::SERVICE_UNAVAILABLE,
                            format!("cannot write into {drop_root}: {e}"),
                        );
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = tokio::fs::remove_file(&temp_path).await;
                    let status = if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
                        StatusCode::PAYLOAD_TOO_LARGE
                    } else {
                        StatusCode::UNPROCESSABLE_ENTITY
                    };
                    return detail_err(status, format!("truncated upload: {e}"));
                }
            }
        }
        if let Err(e) = file.flush().await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return detail_err(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("cannot write into {drop_root}: {e}"),
            );
        }
        wrote = Some(total);
        break;
    }
    let Some(bytes_written) = wrote else {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "a file part named 'file' is required",
        );
    };
    finish_delivery(
        &client,
        &principal.actor,
        &StagedDelivery {
            org: &org,
            org_id: &org_row.id,
            source,
            delivery_id: &delivery_id,
            temp_path: &temp_path,
            bytes: bytes_written,
            action: "import.upload",
        },
    )
    .await
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

/// Viewer+: the most recent ingest-batch receipts
/// (`GET /v1/orgs/{org}/imports/recent`), newest first, a bare array — the
/// shape `runs_list` already uses for the same kind of latest-activity read.
///
/// Separate from `imports_list` rather than a flag on it, because that endpoint
/// is a forward keyset cursor (`id > $after` ascending) and a descending page
/// under `after` semantics has no coherent meaning. The receipts panel read the
/// cursor endpoint with `?limit=20` and therefore rendered the *oldest* 20
/// deliveries; past 20 ingest batches a tenant could never see a new source
/// land on the page that exists to tell them whether it did.
async fn imports_recent(
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
    let snapshot = match read::latest_snapshot(&session) {
        Ok(s) => s,
        Err(e) => return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}")),
    };
    let limit = read::parse_limit(params.get("limit").map(String::as_str));
    match read::imports_recent(
        &session,
        snapshot,
        params.get("source").map(String::as_str),
        limit,
    ) {
        Ok(items) => Json(json!(items)).into_response(),
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
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

/// Viewer+: one run's entity events
/// (`GET /v1/orgs/{org}/runs/{run_id}/entity-events`). Keyset page; query
/// `event_type`, `cursor`, `limit`.
///
/// The detail behind the `entity.*` webhook aggregate. Those events carry a
/// count per kind rather than a row per entity — a run over a large corpus
/// writes millions of ledger rows, and O3a left entity events out of the
/// catalog precisely because a POST each would be worse than none. That trade
/// is only lossless if the rows stay retrievable, which is this endpoint: the
/// aggregate's `detail_path` points here, already narrowed to its kind.
async fn run_entity_events(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, run_id)): Path<(String, String)>,
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
    match read::entity_events_by_run(
        &session,
        snapshot,
        &run_id,
        params
            .get("event_type")
            .map(String::as_str)
            .filter(|v| !v.is_empty()),
        after.as_deref(),
        limit,
    ) {
        Ok((items, next_cursor)) => Json(json!({
            "items": items,
            "run_id": run_id,
            "snapshot": snapshot,
            "next_cursor": next_cursor,
        }))
        .into_response(),
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
    // Via the org row, so the version lookup gets the org id. Passing the NAME
    // here made this return "no published config" for every org that had one,
    // which broke the whole `source_priority` policy.
    let org_row = org_or_404(&client, org).await?;
    let config = org_active_config(&client, &org_row).await?;
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
///
/// Public so `tests/dispatch_coverage.rs` can check every one of them is
/// actually routed. `train` sat in this list with no dispatcher arm, failing
/// every time it fired; a list of kinds the scheduler will enqueue is a promise
/// that something handles them.
pub const SCHEDULABLE_KINDS: [&str; 5] = [
    "run_all_full",
    "run_all_incremental",
    "correct",
    "train",
    "lake_maintain",
];

/// Whether a cron expression is one the scheduler can actually run.
///
/// This defers to [`cron::Cron::parse`], the same parser the dispatcher's tick
/// uses, so the API cannot accept a schedule that would never fire. The previous
/// check was charset-only and happily stored `60 * * * *` and `*/0 * * * *`.
fn valid_cron(s: &str) -> bool {
    cron::Cron::parse(s).is_ok()
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
    use rand::Rng;
    let mut buf = [0u8; 24];
    rand::rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

/// Viewer+: the event types this system can emit
/// (`GET /v1/orgs/{org}/webhooks/events`).
///
/// Discovery, so a tenant wiring up a subscription can see what exists instead
/// of guessing. It is the same list registration validates against, so the two
/// cannot disagree.
async fn webhook_events_catalog(
    principal: Principal,
    Path(org): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    Json(json!({ "events": webhook::catalog_json() })).into_response()
}

/// Viewer+: the segments the active config declares
/// (`GET /v1/orgs/{org}/segments`).
///
/// The same discovery reason as the webhook catalog: a run names a segment, so
/// the names have to be obtainable. Returning the predicate alongside each name
/// is deliberate — it is the tenant's own published config, and an operator
/// choosing how to split a long run needs to see what each slice covers.
async fn segments_catalog(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return Json(json!({ "segments": [] })).into_response();
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let config = match org_active_config(&client, &org_row).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let segments: Vec<serde_json::Value> = config
        .segments
        .iter()
        .map(|(name, predicate)| json!({ "name": name, "predicate": predicate }))
        .collect();
    Json(json!({ "segments": segments })).into_response()
}

/// Viewer+: the delivery log (`GET /v1/orgs/{org}/webhooks/deliveries`),
/// newest first, optionally narrowed by `webhook_id` or `state`.
///
/// This answers the question the old inline-and-discard path could not: did my
/// endpoint actually receive the event, and if not, why. Every emit site used
/// to POST inline and drop the result with `let _ =`, so a subscriber that was
/// unreachable for a moment missed the event permanently and silently.
///
/// The payload is included: it is the tenant's own event body, and seeing what
/// was actually sent is most of the value of a delivery log. The subscriber's
/// SECRET is not — it lives on the webhook row and is returned once, at
/// registration, like an API key.
async fn webhook_deliveries(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Query(q): Query<DeliveryQuery>,
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
    if let Some(bad) = q
        .state
        .as_deref()
        .filter(|s| !["pending", "delivered", "failed"].contains(s))
    {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("unknown state '{bad}' (pending, delivered, failed)"),
        );
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    match er_entities::webhook_delivery::read::page(
        &client,
        &org_row.id,
        q.webhook_id.as_deref(),
        q.state.as_deref(),
        limit,
    )
    .await
    {
        Ok(rows) => Json(json!({ "deliveries": rows })).into_response(),
        Err(e) => pg_err(e),
    }
}

#[derive(Deserialize)]
struct DeliveryQuery {
    webhook_id: Option<String>,
    state: Option<String>,
    limit: Option<i64>,
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
    // The outbound-URL guard. Without it an admin in any tenant could aim the
    // control plane at cloud instance metadata or a cluster-internal service
    // and the dispatcher would POST job payloads to it — the egress policy is
    // deliberately unrestricted, so nothing else was stopping this. Refused
    // here so a bad URL never reaches the table; the delivery path re-checks,
    // because DNS can change between the two.
    if let Err(rejection) = urlguard::check(&body.url, urlguard::Egress::Guarded).await {
        return detail_err(StatusCode::UNPROCESSABLE_ENTITY, rejection.to_string()).into_response();
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
    // Validate against the catalog. Previously `events` was a free-form array
    // and `webhooks_for_delivery` merely tested containment, so a subscription
    // to a non-existent type (or a typo like `job.succeeded`) was accepted and
    // then never delivered to — a silence nobody could diagnose.
    let unknown: Vec<&str> = body
        .events
        .iter()
        .map(String::as_str)
        .filter(|e| !webhook::is_known_event(e))
        .collect();
    if !unknown.is_empty() {
        let known: Vec<&str> = webhook::EVENT_CATALOG.iter().map(|(n, _)| *n).collect();
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "unknown event type(s) {}; this system emits {} (see \
                 GET /v1/orgs/{{org}}/webhooks/events)",
                unknown.join(", "),
                known.join(", ")
            ),
        );
    }
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
#[derive(Deserialize)]
struct TagIn {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

/// Steward+: the tenant's tag vocabulary, or the tags on one subject
/// (`GET /v1/orgs/{org}/tags`, optionally `?subject_type=&subject_id=`).
///
/// One route for both because a caller rendering chips and a caller rendering a
/// filter want the same shape; the subject pair narrows it. Viewer+, since
/// seeing how work is labelled is not changing it.
async fn tags_index(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "tags require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let subject = (
        params.get("subject_type").map(String::as_str),
        params.get("subject_id").map(String::as_str),
    );
    let found = match subject {
        (Some(kind), Some(id)) => {
            if !pg::is_tag_subject(kind) {
                return detail_err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("subject_type must be one of {:?}", pg::TAG_SUBJECT_KINDS),
                );
            }
            pg::tags_for_subject(&client, &org_row.id, kind, id).await
        }
        // Half a subject is a caller bug, not a request for the whole
        // vocabulary: answering the unfiltered list would look like "this thing
        // has every tag".
        (Some(_), None) | (None, Some(_)) => {
            return detail_err(
                StatusCode::UNPROCESSABLE_ENTITY,
                "subject_type and subject_id must be given together",
            );
        }
        (None, None) => pg::list_tags(&client, &org_row.id).await,
    };
    match found {
        Ok(tags) => Json(json!({ "tags": tags })).into_response(),
        Err(e) => pg_err(e),
    }
}

/// Steward+: add a label to the vocabulary (`POST /v1/orgs/{org}/tags`).
///
/// Steward rather than admin: naming the work is part of doing it, and a
/// steward who cannot create a tag will put the distinction in a note field
/// where nothing can filter on it.
async fn create_tag(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<TagIn>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    let name = body.name.trim();
    if name.is_empty() {
        return detail_err(StatusCode::UNPROCESSABLE_ENTITY, "name is required");
    }
    if name.len() > 100 {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "name is at most 100 characters",
        );
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "tags require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let _ = pg::set_actor(&client, &principal.actor).await;
    let description = body
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty());
    match pg::create_tag(&client, &org_row.id, name, description, &principal.actor).await {
        Ok(Some(tag)) => (StatusCode::CREATED, Json(json!(tag))).into_response(),
        // The partial unique index is what rejects a duplicate, so a race
        // between two stewards naming the same tag ends here rather than
        // creating two tags one filter cannot tell apart.
        Ok(None) => detail_err(
            StatusCode::CONFLICT,
            format!("a tag named {name:?} already exists"),
        ),
        Err(e) => pg_err(e),
    }
}

/// Admin+: retire a tag (`DELETE /v1/orgs/{org}/tags/{tag_id}`).
///
/// Admin rather than steward, because retiring affects every subject the tag
/// was applied to and every filter that names it — a wider blast radius than
/// applying one. A SOFT delete: see the entity's `active` column for why a hard
/// one would fail for exactly the tags that had been used.
async fn retire_tag(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, tag_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Admin) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "tags require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::retire_tag(&client, &org_row.id, &tag_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => detail_err(StatusCode::NOT_FOUND, "no such tag"),
        Err(e) => pg_err(e),
    }
}

#[derive(Deserialize)]
struct TagApplyIn {
    subject_type: String,
    subject_id: String,
}

/// Steward+: apply or remove a tag
/// (`POST /v1/orgs/{org}/tags/{tag_id}/apply` and `.../unapply`).
///
/// Two routes onto one handler because the bodies and the checks are identical
/// and only the verb differs; `apply` is idempotent at the schema, so a repeat
/// is 204 rather than a duplicate row.
async fn apply_tag(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, tag_id)): Path<(String, String)>,
    Json(body): Json<TagApplyIn>,
) -> impl IntoResponse {
    tag_application(principal, state, org, tag_id, body, true).await
}

async fn unapply_tag(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, tag_id)): Path<(String, String)>,
    Json(body): Json<TagApplyIn>,
) -> impl IntoResponse {
    tag_application(principal, state, org, tag_id, body, false).await
}

async fn tag_application(
    principal: Principal,
    state: AppState,
    org: String,
    tag_id: String,
    body: TagApplyIn,
    apply: bool,
) -> axum::response::Response {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    if !pg::is_tag_subject(&body.subject_type) {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("subject_type must be one of {:?}", pg::TAG_SUBJECT_KINDS),
        );
    }
    if body.subject_id.trim().is_empty() {
        return detail_err(StatusCode::UNPROCESSABLE_ENTITY, "subject_id is required");
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "tags require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let _ = pg::set_actor(&client, &principal.actor).await;
    let outcome = if apply {
        pg::apply_tag(
            &client,
            &org_row.id,
            &tag_id,
            &body.subject_type,
            &body.subject_id,
            &principal.actor,
        )
        .await
    } else {
        pg::unapply_tag(
            &client,
            &org_row.id,
            &tag_id,
            &body.subject_type,
            &body.subject_id,
        )
        .await
    };
    match outcome {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        // On apply this is "no such tag"; on unapply, "it was not applied".
        // Both are 404 on the thing the caller named.
        Ok(false) => detail_err(
            StatusCode::NOT_FOUND,
            if apply {
                "no such tag"
            } else {
                "that tag is not applied to that subject"
            },
        ),
        Err(e) => pg_err(e),
    }
}

/// Viewer+: what carries a tag (`GET /v1/orgs/{org}/tags/{tag_id}/subjects`).
async fn tag_subjects(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, tag_id)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    if let Some(kind) = params.get("subject_type")
        && !pg::is_tag_subject(kind)
    {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("subject_type must be one of {:?}", pg::TAG_SUBJECT_KINDS),
        );
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "tags require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let limit = read::parse_limit(params.get("limit").map(String::as_str));
    match pg::subjects_for_tag(
        &client,
        &org_row.id,
        &tag_id,
        params.get("subject_type").map(String::as_str),
        limit,
    )
    .await
    {
        Ok(rows) => Json(json!({ "subjects": rows })).into_response(),
        Err(e) => pg_err(e),
    }
}

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
/// The staged response for a call that staged SEVERAL actions (O6).
///
/// A sibling of [`staged_response`] rather than a widening of it: three other
/// handlers stage exactly one action, and their response shape is read by the
/// frontend and pinned by tests. `action_id` is carried for the callers that
/// predate the list — it is the first of `action_ids`, which is the whole
/// truth.
fn staged_batch_response(status: StatusCode, action_ids: &[String]) -> axum::response::Response {
    (
        status,
        Json(json!({
            "status": "staged",
            "action_id": action_ids.first(),
            "action_ids": action_ids,
            "count": action_ids.len(),
            "pending_until_next_reconcile": false,
        })),
    )
        .into_response()
}

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
    if let Err(e) = refuse_unless_active(&org_row, "assertions") {
        return *e;
    }
    match pg::enqueue(
        &client,
        &org_row.id,
        "apply_corrections",
        0,
        Some(&idem),
        Some(&params),
        Some(&principal.actor),
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
    /// One record. Kept alongside `record_keys` because every existing caller
    /// sends this shape and a steward scripting against the API should not have
    /// to change to stay working.
    #[serde(default)]
    record_key: Option<String>,
    /// Several records in ONE call (O6). The point is not convenience: each
    /// call with `apply_now` enqueues an `apply_corrections` job, and that job
    /// runs a correction pass over the whole corpus — so suppressing 500
    /// records a call at a time queues 500 full passes. The frontend's entity
    /// view loops exactly like that today.
    #[serde(default)]
    record_keys: Vec<String>,
    #[serde(default)]
    apply_now: bool,
}

/// How many records one overlay call may carry.
///
/// The same ceiling the mass-update path uses, and surfaced the same way: over
/// it is a 422 naming the cap, never a silent truncation. A caller that thinks
/// it suppressed 3,000 records and actually suppressed 2,000 has no way to find
/// out which 1,000 are still live.
const MAX_OVERLAY_KEYS: usize = 2000;

impl SuppressIn {
    /// The requested keys, de-duplicated, in the order given.
    ///
    /// Order-preserving rather than sorted so the staged actions read back in
    /// the order the steward chose, and de-duplicated because staging the same
    /// overlay twice is two audit rows describing one decision.
    fn keys(&self) -> Vec<String> {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        self.record_keys
            .iter()
            .chain(self.record_key.iter())
            .map(|k| k.trim())
            .filter(|k| !k.is_empty())
            .filter(|k| seen.insert(k))
            .map(ToString::to_string)
            .collect()
    }
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
    let keys = body.keys();
    if keys.is_empty() {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "record_key or a non-empty record_keys is required",
        );
    }
    if keys.len() > MAX_OVERLAY_KEYS {
        // Named, never truncated. A caller that thinks it suppressed 3,000
        // records and actually suppressed 2,000 has no way to find out which
        // 1,000 are still live.
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "{} records requested; at most {MAX_OVERLAY_KEYS} per call — split the batch",
                keys.len()
            ),
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
    // One staged action per record: each is independently auditable and
    // independently retractable, and the drain applies them one at a time
    // regardless. What the batch SHARES is the apply, below.
    let mut action_ids: Vec<String> = Vec::with_capacity(keys.len());
    for record_key in &keys {
        let action = if retract {
            json!({
                "type": "retract_record_overlay",
                "record_key": record_key,
                "kind": overlay,
                "created_by": principal.actor,
            })
        } else {
            json!({
                "type": format!("{overlay}_record"),
                "record_key": record_key,
                "created_by": principal.actor,
            })
        };
        match steward_stage(&client, &principal.actor, &org, &action).await {
            Ok(id) => action_ids.push(id),
            Err(r) => {
                // Partway through, the actions already staged are REAL and will
                // drain. Reporting only "failed" would leave a caller retracting
                // nothing and re-staging everything.
                if action_ids.is_empty() {
                    return *r;
                }
                return detail_err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!(
                        "staged {} of {} records before failing; the staged ones will still apply",
                        action_ids.len(),
                        keys.len()
                    ),
                );
            }
        }
    }
    if !body.apply_now {
        return staged_batch_response(StatusCode::CREATED, &action_ids);
    }
    let idem = format!("{overlay}-apply:{}", er_ids::new_v7());
    let params = json!({ "skip_ingest": true });
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    if let Err(e) = refuse_unless_active(&org_row, "steward actions") {
        return *e;
    }
    match pg::enqueue(
        &client,
        &org_row.id,
        "apply_corrections",
        0,
        Some(&idem),
        Some(&params),
        Some(&principal.actor),
    )
    .await
    {
        // ONE apply job for the whole batch, however many records it carried.
        // That is the point of taking a list: `apply_corrections` runs a
        // correction pass over the entire corpus, so N calls mean N full passes
        // to express one decision.
        Ok(job) => (
            StatusCode::CREATED,
            Json(json!({
                "status": "staged",
                "action_id": action_ids.first(),
                "action_ids": action_ids,
                "count": keys.len(),
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
        Some(&principal.actor),
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
            Some(&principal.actor),
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
            Some(&principal.actor),
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
    // Read before `state` is moved into `with_state`: the import route's body
    // limit is a layer, built here rather than per request.
    let max_import_bytes = state.max_import_bytes;
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
        .route("/v1/orgs/{org}/models", get(models_index))
        .route("/v1/orgs/{org}/models/versions", post(create_model_version))
        // A sub-path, not `models:compile`: matchit cannot match a param and a
        // literal in one segment. The app()-level rewrite accepts the BFF's
        // `:compile` spelling and lands it here.
        .route("/v1/orgs/{org}/models/compile", post(compile_model_version))
        .route(
            "/v1/orgs/{org}/models/{name}/versions/{version}",
            get(get_model_version),
        )
        .route(
            "/v1/orgs/{org}/models/{name}/versions/{version}/evaluation",
            get(model_version_evaluation),
        )
        .route(
            "/v1/orgs/{org}/models/{name}/versions/{version}/activate",
            post(activate_model_version),
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
        .route("/v1/orgs/{org}/blocking-estimate", get(blocking_estimate))
        .route("/v1/orgs/{org}/resolve", post(resolve_record))
        .route("/v1/orgs/{org}/records/ingest", post(push_records))
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
        // The upload needs its own body limit: axum's default (2 MiB) would
        // reject every real delivery, and the limit must be enforced by the
        // server rather than trusted from the BFF.
        .route(
            "/v1/orgs/{org}/imports",
            get(imports_list)
                .post(upload_import)
                .layer(axum::extract::DefaultBodyLimit::max(max_import_bytes)),
        )
        .route("/v1/orgs/{org}/imports/recent", get(imports_recent))
        .route("/v1/orgs/{org}/runs", get(runs_list))
        .route(
            "/v1/orgs/{org}/runs/{run_id}/entity-events",
            get(run_entity_events),
        )
        .route("/v1/orgs/{org}/tags", get(tags_index).post(create_tag))
        .route(
            "/v1/orgs/{org}/tags/{tag_id}",
            axum::routing::delete(retire_tag),
        )
        .route("/v1/orgs/{org}/tags/{tag_id}/apply", post(apply_tag))
        .route("/v1/orgs/{org}/tags/{tag_id}/unapply", post(unapply_tag))
        .route("/v1/orgs/{org}/tags/{tag_id}/subjects", get(tag_subjects))
        .route("/v1/orgs/{org}/merge-plans", get(merge_plans))
        .route(
            "/v1/orgs/{org}/schedules",
            get(schedules_index).post(create_schedule),
        )
        .route("/v1/orgs/{org}/labeling/next", get(labeling_next))
        .route("/v1/orgs/{org}/labels", post(create_label).get(labels_list))
        .route("/v1/orgs/{org}/insights", get(insights))
        .route("/v1/orgs/{org}/insights/sources", get(insight_sources))
        .route("/v1/orgs/{org}/quality", get(quality))
        .route("/v1/orgs/{org}/segments", get(segments_catalog))
        .route(
            "/v1/orgs/{org}/webhooks/deliveries",
            get(webhook_deliveries),
        )
        .route(
            "/v1/orgs/{org}/webhooks/events",
            get(webhook_events_catalog),
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
const ACTION_VERBS: [&str; 11] = [
    "unmerge",
    "resolve",
    "cancel",
    "resume",
    "publish",
    "bulk-resolve",
    "contradictions",
    "suspend",
    // The model registry's two actions, so `models:compile` and
    // `.../versions/3:activate` reach their `/compile` and `/activate` routes.
    "activate",
    "compile",
    // `records:ingest`, the JSON push.
    "ingest",
];

/// Rewrite a Python-style `{id}:action` path into the `{id}/action` sub-path this
/// server routes on. Splits the LAST segment on its LAST `:` and only rewrites
/// when the suffix is one of the known action verbs — so an id that itself contains
/// colons (an unencoded `review_id` like `gray_band:crm:C001:webforms:W006`) is
/// left alone, while the BFF's encoded `{id}:action` (the id's own colons arrive
/// as `%3A`) is normalized. Returns `None` when no rewrite applies.
///
/// Public so the mock-contract gate (`tests/mock_contract.rs`) can normalize the
/// frontend mock's `:action` spellings through this exact rule instead of a copy
/// of it that could drift.
pub fn rewrite_action_path(path: &str) -> Option<String> {
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
    state
        .with_mail(mail::Mailer::from_env().await)
        .with_provision(provision::Profile::from_env())
}

#[cfg(test)]
mod evaluation_tests {
    use super::{confusion_metrics, label_evaluation};
    use er_entities::pair_label::PairLabel;

    /// A label on an arbitrary pair. `score` of `None` is a label recorded with
    /// no score in front of the labeller.
    fn label(
        verdict: &str,
        origin: &str,
        model_version: Option<&str>,
        score: Option<f64>,
    ) -> PairLabel {
        PairLabel {
            id: format!("l-{verdict}-{origin}-{score:?}"),
            org_id: "o1".into(),
            rec_a_key: "a".into(),
            rec_b_key: "b".into(),
            verdict: verdict.into(),
            origin: origin.into(),
            labeled_by: "steward@example.com".into(),
            model_version: model_version.map(ToString::to_string),
            score_at_label: score,
            note: None,
            // Fixed rather than `now()`: nothing here reads the timestamps, and
            // a constant keeps the fixture deterministic.
            created_at: "2026-01-01T00:00:00Z".parse().expect("a fixed timestamp"),
            updated_at: "2026-01-01T00:00:00Z".parse().expect("a fixed timestamp"),
        }
    }

    #[test]
    fn a_ratio_with_no_denominator_is_null_not_zero() {
        // The distinction a steward acts on: "got every positive wrong" vs
        // "made no positive predictions at all".
        let m = confusion_metrics(0, 0, 3, 5);
        assert!(m["precision"].is_null(), "{m}");
        assert_eq!(m["recall"], serde_json::json!(0.0));
        assert!(m["f1"].is_null(), "{m}");
        let empty = confusion_metrics(0, 0, 0, 0);
        assert!(empty["accuracy"].is_null(), "{empty}");
    }

    #[test]
    fn a_perfect_separation_scores_one() {
        let labels = vec![
            label("match", "human", Some("v1"), Some(0.99)),
            label("match", "human", Some("v1"), Some(0.96)),
            label("no_match", "human", Some("v1"), Some(0.10)),
        ];
        let out = label_evaluation(&labels, "v1", 0.95);
        assert_eq!(out["overall"]["precision"], serde_json::json!(1.0));
        assert_eq!(out["overall"]["recall"], serde_json::json!(1.0));
        assert_eq!(out["overall"]["f1"], serde_json::json!(1.0));
        assert_eq!(out["counts"]["evaluable"], serde_json::json!(3));
    }

    #[test]
    fn the_threshold_moves_the_matrix() {
        let labels = vec![
            label("match", "human", Some("v1"), Some(0.80)),
            label("no_match", "human", Some("v1"), Some(0.70)),
        ];
        // At 0.95 neither is predicted a match: one false negative, one true
        // negative, and no positive predictions at all.
        let strict = label_evaluation(&labels, "v1", 0.95);
        assert!(strict["overall"]["precision"].is_null(), "{strict}");
        assert_eq!(strict["overall"]["fn"], serde_json::json!(1));
        // At 0.5 both are: one true positive, one false positive.
        let loose = label_evaluation(&labels, "v1", 0.5);
        assert_eq!(loose["overall"]["tp"], serde_json::json!(1));
        assert_eq!(loose["overall"]["fp"], serde_json::json!(1));
        assert_eq!(loose["overall"]["precision"], serde_json::json!(0.5));
    }

    #[test]
    fn another_versions_labels_are_not_counted() {
        // The trap this guards: filtering on the integer `version` instead of
        // the `model_version_label` matches nothing, and a run of this endpoint
        // would report an honest but useless "0 labels" for every version.
        let labels = vec![
            label("match", "human", Some("v1"), Some(0.99)),
            label("no_match", "human", Some("v2"), Some(0.99)),
        ];
        let out = label_evaluation(&labels, "v1", 0.95);
        assert_eq!(out["counts"]["labels_for_version"], serde_json::json!(1));
        assert_eq!(out["overall"]["tp"], serde_json::json!(1));
        assert_eq!(out["overall"]["fp"], serde_json::json!(0));
    }

    #[test]
    fn an_unscored_label_is_excluded_and_counted() {
        // Every `reviewer`-origin label is unscored by construction (resolving a
        // review has no probability in front of the human), so this is the
        // normal case rather than an edge one.
        let labels = vec![
            label("match", "human", Some("v1"), Some(0.99)),
            label("match", "reviewer", Some("v1"), None),
            label("dismiss", "human", Some("v1"), Some(0.40)),
        ];
        let out = label_evaluation(&labels, "v1", 0.95);
        assert_eq!(out["counts"]["labels_for_version"], serde_json::json!(3));
        assert_eq!(out["counts"]["evaluable"], serde_json::json!(1));
        assert_eq!(out["counts"]["no_score"], serde_json::json!(1));
        assert_eq!(out["counts"]["not_a_verdict"], serde_json::json!(1));
    }

    #[test]
    fn metrics_are_split_by_origin() {
        // E10b's rule: an accuracy number must say whose labels it was measured
        // against. An origin with no evaluable label is absent rather than
        // present-and-null, so the reader cannot mistake it for a measurement.
        let labels = vec![
            label("match", "human", Some("v1"), Some(0.99)),
            label("no_match", "llm", Some("v1"), Some(0.99)),
        ];
        let out = label_evaluation(&labels, "v1", 0.95);
        assert_eq!(
            out["by_origin"]["human"]["precision"],
            serde_json::json!(1.0)
        );
        assert_eq!(out["by_origin"]["llm"]["precision"], serde_json::json!(0.0));
        assert!(out["by_origin"].get("reviewer").is_none(), "{out}");
        // The overall figure mixes them, which is why the split exists.
        assert_eq!(out["overall"]["precision"], serde_json::json!(0.5));
    }

    #[test]
    fn the_curve_spans_the_range_and_is_monotone_in_recall() {
        let labels = vec![
            label("match", "human", Some("v1"), Some(0.9)),
            label("match", "human", Some("v1"), Some(0.5)),
            label("no_match", "human", Some("v1"), Some(0.1)),
        ];
        let out = label_evaluation(&labels, "v1", 0.95);
        let curve = out["curve"].as_array().expect("curve").clone();
        assert_eq!(curve.len(), 21);
        assert_eq!(curve[0]["threshold"], serde_json::json!(0.0));
        assert_eq!(curve[20]["threshold"], serde_json::json!(1.0));
        // Raising the threshold can only predict fewer matches, so recall is
        // non-increasing. A curve that wobbles means the matrix is wrong.
        let recalls: Vec<f64> = curve
            .iter()
            .map(|p| p["recall"].as_f64().unwrap_or(0.0))
            .collect();
        for pair in recalls.windows(2) {
            assert!(
                pair[1] <= pair[0],
                "recall rose across thresholds: {recalls:?}"
            );
        }
    }

    #[test]
    fn no_labels_for_a_version_reports_nothing_rather_than_zero() {
        let out = label_evaluation(&[], "v1", 0.95);
        assert_eq!(out["counts"]["evaluable"], serde_json::json!(0));
        assert!(out["overall"]["precision"].is_null(), "{out}");
        assert!(out["overall"]["accuracy"].is_null(), "{out}");
    }
}

#[cfg(test)]
mod tests {
    use super::{ACTING_USER_MAX, compose_actor, forwarded_actor, rewrite_action_path};

    /// Build request parts carrying the given `x-acting-user` value.
    fn parts_with(header: Option<&str>) -> axum::http::request::Parts {
        let mut builder = axum::http::Request::builder().uri("/");
        if let Some(value) = header {
            builder = builder.header(super::ACTING_USER_HEADER, value);
        }
        builder.body(()).unwrap().into_parts().0
    }

    #[test]
    fn the_recorded_actor_names_the_person_and_the_credential() {
        // The format `proxy.ts` documents: "user:<email> via key:…". Both halves
        // matter — the person is who to ask about a decision, the credential is
        // how the request got in, and a forged name cannot hide the latter.
        assert_eq!(
            compose_actor("key:01ABC", Some("steward@acme.dev".to_string())),
            "user:steward@acme.dev via key:01ABC"
        );
        assert_eq!(
            compose_actor("operator", Some("root@er.dev".to_string())),
            "user:root@er.dev via operator"
        );
        // No header: exactly the previous behaviour, so a direct API caller's
        // audit rows are unchanged.
        assert_eq!(compose_actor("key:01ABC", None), "key:01ABC");
        assert_eq!(compose_actor("operator", None), "operator");
    }

    #[test]
    fn an_unusable_acting_user_is_ignored_rather_than_trusted() {
        assert_eq!(
            forwarded_actor(&parts_with(Some("steward@acme.dev"))).as_deref(),
            Some("steward@acme.dev")
        );
        // Surrounding whitespace is trimmed, not rejected.
        assert_eq!(
            forwarded_actor(&parts_with(Some("  steward@acme.dev  "))).as_deref(),
            Some("steward@acme.dev")
        );
        // Absent, empty, or whitespace-only: fall back to the credential.
        assert!(forwarded_actor(&parts_with(None)).is_none());
        assert!(forwarded_actor(&parts_with(Some(""))).is_none());
        assert!(forwarded_actor(&parts_with(Some("   "))).is_none());
        // A space would let a caller pass off "x via key:other" as one actor,
        // and a quote or backslash would need escaping wherever the actor is
        // embedded. Refused rather than sanitised: a name that needed rewriting
        // is not a name worth recording. Asserted through a real header, since
        // these are all values a header may legally carry.
        for bad in ["ada admin", "ada\tadmin", "\"ada\"", "ada\\admin"] {
            assert!(
                forwarded_actor(&parts_with(Some(bad))).is_none(),
                "{bad:?} must not reach the audit trail"
            );
        }
        // Bounded: an email, not an essay.
        let long = "a".repeat(ACTING_USER_MAX + 1);
        assert!(forwarded_actor(&parts_with(Some(&long))).is_none());
        let at_limit = "a".repeat(ACTING_USER_MAX);
        assert!(forwarded_actor(&parts_with(Some(&at_limit))).is_some());
    }

    /// CRLF is refused by the predicate, and the reason it is tested HERE rather
    /// than through a header is itself the finding: `http::Request::builder()`
    /// will not construct a header value containing `\n` or `\r` — it returns
    /// `InvalidHeaderValue` — so log-forging through this header is already
    /// impossible one layer below us. The predicate still rejects them, because
    /// a rule that depends on a neighbour's behaviour for its safety should say
    /// so and then not rely on it.
    #[test]
    fn control_characters_are_refused_though_a_header_cannot_carry_them() {
        for bad in ["ada\nadmin@acme.dev", "ada\radmin", "ada\0admin"] {
            assert!(!super::acceptable_actor(bad), "{bad:?} must be refused");
            assert!(
                axum::http::Request::builder()
                    .uri("/")
                    .header(super::ACTING_USER_HEADER, bad)
                    .body(())
                    .is_err()
                    || !bad.contains(['\n', '\r']),
                "the http layer should refuse to build {bad:?} as a header"
            );
        }
    }

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
