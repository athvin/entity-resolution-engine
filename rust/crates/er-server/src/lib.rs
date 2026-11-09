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
pub mod dags;
pub mod delivery;
pub mod dispatcher;
pub mod dispatcher_bin;
pub mod errfmt;
pub mod exec_task;
pub mod executor;
pub mod extract_params;
pub(crate) mod fixups;
pub mod mail;
pub mod migrate_plan;
pub mod openapi;
pub mod ops;
pub mod pg;
pub mod profile_params;
pub mod provision;
pub mod read;
pub mod runctx;
pub mod s3_ingest;
pub mod salesforce;
pub mod satoken;
pub mod schema;
pub mod shutdown;
pub mod taskpg;
pub mod telemetry;
pub mod urlguard;
pub mod webhook;
pub mod writeback;

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
    /// The `POST /v1/orgs/{org}/datasets/{ds}/imports` body cap, enforced both as the route's
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
    /// The projected-ServiceAccount-token verifier (§6.4), present only when
    /// this process runs as a pod. `None` leaves the attempt channel
    /// fallback-token-only, exactly as before.
    sa_verifier: Option<Arc<satoken::Verifier>>,
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
            sa_verifier: None,
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
            sa_verifier: None,
        }
    }

    /// Attach the projected-ServiceAccount-token verifier (§6.4). Absent, the
    /// attempt channel accepts only the per-attempt fallback token.
    #[must_use]
    pub fn with_sa_verifier(mut self, verifier: Option<satoken::Verifier>) -> Self {
        self.sa_verifier = verifier.map(Arc::new);
        self
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

    /// Override the import body cap: the `POST /v1/orgs/{org}/datasets/{ds}/imports` route's
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

/// An org/dataset env write refusal: a credential-shaped key is the caller's
/// mistake (422, and the message names the key — never a value); a database
/// failure is the usual 500.
fn env_write_err(e: pg::EnvWriteError) -> axum::response::Response {
    match e {
        pg::EnvWriteError::CredentialKey(_) => {
            detail_err(StatusCode::UNPROCESSABLE_ENTITY, e.to_string())
        }
        pg::EnvWriteError::Pg(e) => pg_err(e),
    }
}

/// A `{ "detail": … }` error body — the FastAPI shape the frontend BFF parses
/// (`proxy.ts` reads `detail`, and keys its user-facing mapping on the status).
fn detail_err(code: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (code, Json(json!({ "detail": msg.into() }))).into_response()
}

/// A landing gate's refusal: the response to send, plus the reason in words
/// when the gate itself raised it.
///
/// `delivery_preconditions` has two kinds of caller now. The HTTP handlers want
/// the response, byte for byte as before. E9c's poller wants the *reason*,
/// because a skipped auto-ingest has no reader but a log line, and "the gate
/// refused with 422" tells a tenant nothing about which of their files was
/// ignored or why.
///
/// `detail` is `Option` rather than always-present on purpose, and the
/// alternative is why: carrying a reason for every refusal would mean giving
/// `org_or_404` and `org_active_config` this type too, and those have 43 and 10
/// callers. A refusal passed through from one of them arrives already rendered
/// and keeps its exact body — the poller falls back to the status for those,
/// and they are the refusals it cannot reach anyway (the org was listed
/// moments earlier).
pub(crate) struct Refusal {
    response: axum::response::Response,
    detail: Option<String>,
}

impl Refusal {
    fn new(code: StatusCode, detail: impl Into<String>) -> Box<Self> {
        let detail = detail.into();
        Box::new(Self {
            response: detail_err(code, detail.clone()),
            detail: Some(detail),
        })
    }

    /// The reason in words, for a caller with nowhere to send a response.
    pub(crate) fn reason(&self) -> String {
        self.detail
            .clone()
            .unwrap_or_else(|| format!("refused with {}", self.response.status()))
    }
}

impl From<Box<axum::response::Response>> for Box<Refusal> {
    fn from(response: Box<axum::response::Response>) -> Self {
        Box::new(Refusal {
            response: *response,
            detail: None,
        })
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> axum::response::Response {
        self.response
    }
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
/// [`refuse_unless_active`] down to the dataset: the org must be active AND
/// the dataset must be active, the 409 naming whichever is not. A suspended
/// dataset refuses work without touching its siblings — the point of
/// per-dataset lifecycle.
fn refuse_unless_active_dataset(
    org: &pg::OrgDetail,
    d: &pg::DatasetDetail,
    what: &str,
) -> Result<(), Box<axum::response::Response>> {
    refuse_unless_active(org, what)?;
    if d.state == "active" {
        return Ok(());
    }
    Err(Box::new(detail_err(
        StatusCode::CONFLICT,
        format!(
            "dataset '{}' is {}; {what} are accepted only when active",
            d.name, d.state
        ),
    )))
}

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
    /// The caller's W3C trace context, if it sent a well-formed one. Carried
    /// here because this is already the one thing extracted from headers on
    /// every authenticated request, so a handler that enqueues a job has the
    /// context in scope without new plumbing.
    ///
    /// Correlation only — never an authorization input. It is spoofable exactly
    /// as the acting-user header is, which is tolerable because nothing
    /// downstream makes a decision on a trace id; `forwarded_traceparent` in
    /// this module carries the full reasoning.
    pub trace_context: Option<String>,
}

impl Principal {
    /// This caller's job provenance: the recorded actor plus the trace context
    /// it arrived with. The one place a [`pg::Provenance`] is built from a
    /// request — everything else is [`pg::Provenance::default`] (no request) or
    /// [`pg::Provenance::actor`] (an internal caller that names itself).
    #[must_use]
    pub fn provenance(&self) -> pg::Provenance<'_> {
        pg::Provenance {
            created_by: Some(&self.actor),
            trace_context: self.trace_context.as_deref(),
        }
    }
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

/// The W3C trace-context header. The BFF sends it so a job enqueued by a
/// request can be linked back to the click that asked for it; `jobs.trace_context`
/// stores it verbatim and the dispatcher starts the run's span as its child
/// (infrastructure.md §10.2).
const TRACEPARENT_HEADER: &str = "traceparent";

/// A `traceparent` is exactly `00-<32 hex>-<16 hex>-<2 hex>` — 55 characters.
const TRACEPARENT_LEN: usize = 55;

/// The trace context named by [`TRACEPARENT_HEADER`], if present and well formed.
///
/// Spoofable, exactly as [`forwarded_actor`] is: any credential holder can send
/// any trace id. That is tolerable because a trace id is a correlation handle
/// and never an authorization input — nothing downstream makes a decision on it.
/// What a forged value can do is attach this run to someone else's trace, so the
/// durable attribution stays `created_by`/`audit_log` and this is only ever read
/// for linking.
///
/// Malformed values are ignored rather than refused, for the reason
/// [`forwarded_actor`] gives: a bad header should not fail an otherwise valid
/// request, and a null `trace_context` is exactly the pre-existing behaviour.
fn forwarded_traceparent(parts: &Parts) -> Option<String> {
    forwarded_traceparent_header(&parts.headers)
}

/// [`forwarded_traceparent`] over a bare header map.
///
/// Split out for [`telemetry::make_span`], which makes the request span a child
/// of this context and is handed a `&Request` rather than request `Parts`. One
/// function rather than two readings of the same header, because the span's
/// parent and the value stored on `jobs.trace_context` have to be the *same*
/// context — if one accepted a header the other rejected, the trace would break
/// somewhere other than where it appeared to.
pub(crate) fn forwarded_traceparent_header(headers: &axum::http::HeaderMap) -> Option<String> {
    let raw = headers.get(TRACEPARENT_HEADER)?.to_str().ok()?.trim();
    acceptable_traceparent(raw).then(|| raw.to_string())
}

/// Whether a trimmed `traceparent` is fit to store: the W3C level-1 shape, with
/// an all-zero trace or span id rejected (the spec calls both invalid) and
/// version `ff` rejected (the spec forbids it).
///
/// Separate from [`forwarded_traceparent`] so the rule is testable without a
/// header, which is the same reason [`acceptable_actor`] is split out. Future
/// versions are deliberately *not* accepted: a `01-` context may carry fields
/// whose meaning we would be guessing at, and storing one we cannot interpret is
/// worse than storing nothing.
fn acceptable_traceparent(raw: &str) -> bool {
    if raw.len() != TRACEPARENT_LEN {
        return false;
    }
    let mut fields = raw.split('-');
    let (Some(version), Some(trace_id), Some(span_id), Some(flags), None) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    let hex = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit());
    // Lowercase only: the spec requires it, and accepting mixed case would let
    // the same context arrive as two distinct strings.
    let lower = |s: &str| s.bytes().all(|b| !b.is_ascii_uppercase());
    hex(version, 2)
        && version == "00"
        && hex(trace_id, 32)
        && trace_id.bytes().any(|b| b != b'0')
        && hex(span_id, 16)
        && span_id.bytes().any(|b| b != b'0')
        && hex(flags, 2)
        && lower(raw)
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
                trace_context: forwarded_traceparent(parts),
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
                    trace_context: forwarded_traceparent(parts),
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

/// Public: liveness (`GET /healthz`) — the only route that never
/// authenticates (§4.1).
///
/// Deliberately a literal: liveness answers "is this process wedged", and a
/// probe that fails on a *dependency* outage restarts a healthy pod and makes
/// the outage worse. Readiness is [`readyz`].
async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

/// Public: readiness (`GET /readyz`) — can this process actually serve?
///
/// The chart's readiness and startup probes read this; liveness keeps
/// [`healthz`]. Both used to point at `/healthz`, which is a static literal, so
/// a Postgres outage left the pod reporting ready while every `/v1` request
/// 500s — the pod stayed in the Service and kept taking traffic it could not
/// serve. This is the probe that answers the question readiness is asking.
///
/// Readiness carries `failureThreshold: 6` in the chart because of the 2 s
/// timeout below: dev Aurora sets `min_capacity = 0`, so the first probe after
/// an idle period times out while the cluster resumes, and at one replica a pod
/// leaving the Service is a hard 503 at the edge. Thirty seconds of consecutive
/// failure is the bar, not one.
///
/// Anonymous, like `/healthz`: a probe has no credential. It therefore reports
/// only a boolean, the backing store and a reason class — never a DSN, a host
/// or an error chain. The endpoint is reachable from anywhere the pod is.
///
/// The three keys are always present, `reason` null when ready. An envelope
/// that changed shape with the outcome is what the generated
/// `frontend/src/lib/api/envelopes.json` ledger caught here, and it was worth
/// catching: a body whose keys depend on success is a body every consumer has
/// to branch on.
async fn readyz(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    // No DSN configured is the in-memory dev/test backend, which is ready by
    // construction. Reporting "not ready" there would break `make dev`.
    let Some(dsn) = state.dsn.clone() else {
        return (
            StatusCode::OK,
            Json(json!({ "ready": true, "store": "memory", "reason": null })),
        );
    };

    // A fresh connect, bounded: this runs every few seconds, so it must never
    // be the thing that hangs. connect-per-request is the house pattern
    // (`pg::connect`), so this is the same cost a real request pays.
    let probe = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let client = pg::connect(&dsn).await?;
        client.simple_query("SELECT 1").await?;
        Ok::<(), er_pg_tls::Error>(())
    })
    .await;

    match probe {
        Ok(Ok(())) => (
            StatusCode::OK,
            Json(json!({ "ready": true, "store": "postgres", "reason": null })),
        ),
        Ok(Err(e)) => {
            // The chain goes to the log, where it is useful and access-controlled;
            // the response carries only the class.
            tracing::warn!(error = %errfmt::chain(&e), "readiness probe: control plane unreachable");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "ready": false, "store": "postgres", "reason": "control-plane" })),
            )
        }
        Err(_) => {
            tracing::warn!("readiness probe: control plane timed out");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(
                    json!({ "ready": false, "store": "postgres", "reason": "control-plane-timeout" }),
                ),
            )
        }
    }
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
                // Same correction as the publish path below: these problems are
                // no longer only about column names.
                "the starter config {} is not valid: {}",
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
        // Auto-provisioning never turns auto-ingest on: a tenant provisioned
        // from a template has pointed us at no bucket, and inventing one would
        // have the poller reading object storage on its own initiative.
        None,
        None,
    )
    .await
    {
        return env_write_err(e);
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

    // The org's one dataset exists from its first moment rather than at the
    // next boot's convergence sweep, so everything below can stamp it.
    if let Err(e) = fixups::ensure_default_dataset_by_name(&client, name).await {
        return detail_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("creating the default dataset: {e}"),
        );
    }
    let ds = stamp_dataset(&client, &org_row.id).await;

    // The starter config as version 1, published so the tenant has an active
    // config from the start — the wizard and the upload path both require one.
    let hash = config_hash(&config);
    let version = match pg::create_config_version(
        &client,
        &org_row.id,
        ds.as_deref(),
        &yaml,
        &hash,
        &principal.actor,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return pg_err(e),
    };
    if let Err(e) = provision::create_workspace(&plan, &profile.config_template) {
        return detail_err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    // (org_id, org_name) — the two are adjacent `&str`s, so a swap compiles and
    // silently publishes nothing. Caught by asserting `active_config_version`.
    let Some(ds_id) = ds.as_deref() else {
        return detail_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "the default dataset was just converged yet cannot be read back",
        );
    };
    if let Err(e) = pg::publish_config_version(&client, name, ds_id, version, Some("C")).await {
        return pg_err(e);
    }

    let (admin_key_id, admin_key) = match pg::issue_key(&client, &org_row.id, "admin").await {
        Ok(pair) => pair,
        Err(e) => return pg_err(e),
    };

    let idem = format!("provision:{name}");
    // Org-level by design: the provision job creates the tenant itself.
    let job = match pg::enqueue(
        &client,
        &org_row.id,
        None,
        "provision",
        0,
        Some(&idem),
        None,
        principal.provenance(),
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
        None,
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
    /// Auto-ingest (E9c): the bucket the dispatcher's poller watches. Absent
    /// leaves auto-ingest off, which is the behaviour every existing caller
    /// already has.
    #[serde(default)]
    s3_ingest_bucket: Option<String>,
    #[serde(default)]
    s3_ingest_prefix: Option<String>,
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
        body.s3_ingest_bucket.as_deref(),
        body.s3_ingest_prefix.as_deref(),
    )
    .await
    {
        Ok(()) => {
            let _ = pg::audit(
                &client,
                &principal.actor,
                Some(&body.name),
                None,
                "org.upsert",
                &json!({}),
            )
            .await;
            // The org's one dataset exists from its first moment rather than
            // at the next boot's convergence sweep.
            if let Err(e) = fixups::ensure_default_dataset_by_name(&client, &body.name).await {
                return detail_err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("creating the default dataset: {e}"),
                );
            }
            let seeded = seed_starter_model(&client, &body.name, &env, &principal.actor).await;
            (
                StatusCode::CREATED,
                Json(json!({ "name": body.name, "starter_model": seeded })),
            )
                .into_response()
        }
        Err(e) => env_write_err(e),
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
    // The starter model belongs to the org's one dataset, converged moments
    // earlier by the create-org handler.
    let Ok(Some(ds_id)) = pg::default_dataset_id(client, &org_row.id).await else {
        return json!({ "seeded": false, "reason": "the org has no default dataset yet" });
    };
    // `POST /v1/orgs` is an idempotent upsert, so a replay must not mint a
    // second version: versions auto-increment, which would otherwise make
    // re-registering an org silently accumulate identical models.
    match pg::list_model_versions(client, &ds_id).await {
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
    let ds = stamp_dataset(client, &org_row.id).await;
    let created = pg::create_model_version(
        client,
        &org_row.id,
        ds.as_deref(),
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
            if let Err(e) = pg::activate_model_version(client, &ds_id, &m.id).await {
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
            tracing::error!(
                er.template = %body.template,
                error = %errfmt::chain(&e),
                "the email transport failed"
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
            let _ = pg::audit(
                &client,
                &principal.actor,
                Some(org),
                None,
                action,
                &json!({}),
            )
            .await;
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

/// Resolve a dataset by its org row and name, or the scope-safe 404 (the same
/// shape a wrong org gets, so probing dataset names learns nothing).
async fn dataset_or_404(
    client: &tokio_postgres::Client,
    org_row: &pg::OrgDetail,
    ds: &str,
) -> Result<pg::DatasetDetail, Box<axum::response::Response>> {
    match pg::get_dataset(client, &org_row.id, ds).await {
        Ok(Some(row)) => Ok(row),
        Ok(None) => Err(Box::new(
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "detail": format!("no dataset '{ds}' in org '{}'", org_row.name) })),
            )
                .into_response(),
        )),
        Err(e) => Err(Box::new(pg_err(e))),
    }
}

/// The tenant-facing dataset shape. `env` and `config_path` stay server-side:
/// they are lake plumbing the engine reads, not product state.
fn dataset_json(d: &pg::DatasetDetail) -> Value {
    json!({
        "name": d.name,
        "display_name": d.display_name,
        "entity_type": d.entity_type,
        "state": d.state,
        "active_config_version": d.active_config_version,
        "created_at": d.created_at,
        "updated_at": d.updated_at,
    })
}

/// The org's default-dataset id, as a best-effort stamp for org-scoped
/// writes. `None` on a read error as well as on a genuinely missing dataset,
/// deliberately: the column is nullable by design, the convergence sweep and
/// the backfill heal NULLs, and failing a whole request over a stamp the
/// schema does not require would invert the feature's priorities. The
/// dataset-scoped routes pass their resolved dataset instead of this.
async fn stamp_dataset(client: &tokio_postgres::Client, org_id: &str) -> Option<String> {
    pg::default_dataset_id(client, org_id).await.ok().flatten()
}

/// Dataset names the frontend's static path segments own; a dataset wearing
/// one would be unroutable in the app even though the API could serve it.
const RESERVED_DATASET_NAMES: &[&str] = &["datasets", "settings"];

/// Viewer+: an org's datasets (`GET /v1/orgs/{org}/datasets`).
async fn list_datasets_route(
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
    let org_row = match org_or_404(&client, &org).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    match pg::list_datasets(&client, &org_row.id).await {
        Ok(rows) => {
            let out: Vec<Value> = rows.iter().map(dataset_json).collect();
            Json(json!(out)).into_response()
        }
        Err(e) => pg_err(e),
    }
}

#[derive(Deserialize)]
struct DatasetIn {
    name: String,
    #[serde(default)]
    display_name: Option<String>,
    /// `person` (default) or `account` — fixed at creation; publish and model
    /// activation enforce agreement from then on.
    #[serde(default)]
    entity_type: Option<String>,
    #[serde(default)]
    config_path: Option<String>,
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    drop_root: Option<String>,
    #[serde(default)]
    s3_ingest_bucket: Option<String>,
    #[serde(default)]
    s3_ingest_prefix: Option<String>,
}

/// Operator: create a dataset (`POST /v1/orgs/{org}/datasets`), bring-your-own
/// config only for now — the caller supplies `config_path` and the lake env.
/// Auto-provisioning (derive the lake, seed a starter config, run the
/// provision job) arrives with the `dataset_provision` job kind; until then an
/// omitted `config_path` is the honest 503, exactly as org auto-provisioning
/// was before it existed.
async fn create_dataset(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<DatasetIn>,
) -> impl IntoResponse {
    // BYO (an explicit config_path + lake env) stays operator-only: it can
    // point a dataset at arbitrary paths, which is operator power. The
    // auto-provisioning path derives everything, so a tenant admin may use it
    // — it is the create-dataset wizard's whole mechanism.
    if body.config_path.is_some() {
        if let Err(e) = operator_only(&principal) {
            return e.into_response();
        }
    } else if let Err(e) = guard(&principal, &org, Role::Admin) {
        return e.into_response();
    }
    if !valid_org_name(&body.name) {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("invalid dataset name '{}'", body.name),
        );
    }
    if RESERVED_DATASET_NAMES.contains(&body.name.as_str()) {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("'{}' is a reserved dataset name", body.name),
        );
    }
    let entity_type = body.entity_type.as_deref().unwrap_or("person");
    if !matches!(entity_type, "person" | "account") {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("entity_type must be 'person' or 'account', not '{entity_type}'"),
        );
    }
    let Some(dsn) = &state.dsn else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({ "detail": "org registry requires Postgres" })),
        )
            .into_response();
    };
    let Some(config_path) = body.config_path.as_deref() else {
        // Auto mode: derive the dataset's lake from the provisioning profile,
        // create its workspace and starter config, insert the row
        // `provisioning`, and hand the slow half (the lake's first attach, the
        // activation) to the dataset_provision job — the same split as org
        // provisioning.
        let profile = match (&state.provision, &state.provision_error) {
            (Some(profile), _) => profile,
            (None, Some(e)) => {
                return detail_err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("auto-provisioning is misconfigured: {e}"),
                );
            }
            (None, None) => {
                return detail_err(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "dataset auto-provisioning not configured (set ER_PROVISION_MODE); \
                     operators may pass config_path and the lake env instead",
                );
            }
        };
        let client = match connect_pg(dsn).await {
            Ok(c) => c,
            Err(r) => return *r,
        };
        let org_row = match org_or_404(&client, &org).await {
            Ok(row) => row,
            Err(r) => return *r,
        };
        if let Err(r) = refuse_unless_active(&org_row, "create a dataset") {
            return *r;
        }
        if let Ok(Some(_)) = pg::get_dataset(&client, &org_row.id, &body.name).await {
            return detail_err(
                StatusCode::CONFLICT,
                format!("dataset '{}' already exists in org '{org}'", body.name),
            );
        }
        let plan = profile.plan_dataset(&org, &body.name);
        if let Err(e) = provision::create_workspace(&plan, &profile.config_template) {
            return detail_err(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
        if matches!(profile.mode, provision::Mode::Local)
            && let Err(e) = provision::create_local_lake(&plan)
        {
            return detail_err(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
        let starter = match provision::starter_config(&profile.config_template) {
            Ok(yaml) => yaml,
            Err(e) => return detail_err(StatusCode::INTERNAL_SERVER_ERROR, e),
        };
        let starter_hash = match load_config_str(&starter) {
            Ok(config) => config_hash(&config),
            Err(e) => {
                return detail_err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!(
                        "the starter config template does not load: {}",
                        errfmt::chain(&e)
                    ),
                );
            }
        };
        let ds_id = er_ids::new_v7();
        let _ = pg::set_actor(&client, &principal.actor).await;
        if let Err(e) = er_entities::dataset::create::ensure(
            &client,
            &ds_id,
            &org_row.id,
            &body.name,
            body.display_name.as_deref(),
            entity_type,
            &plan.config_path.to_string_lossy(),
            &plan.env_json(),
            Some(&plan.drop_root.to_string_lossy()),
            None,
            None,
            None,
            "provisioning",
        )
        .await
        {
            return pg_err(e);
        }
        let version = match pg::create_config_version(
            &client,
            &org_row.id,
            Some(&ds_id),
            &starter,
            &starter_hash,
            &principal.actor,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => return pg_err(e),
        };
        if let Err(e) =
            er_entities::config_version::update::publish(&client, Some("C"), &org_row.id, version)
                .await
        {
            return pg_err(e);
        }
        if let Err(e) =
            er_entities::dataset::update::set_active_config(&client, version, &ds_id).await
        {
            return pg_err(e);
        }
        let idem = format!("dataset-provision:{org}:{}", body.name);
        let job = match pg::enqueue(
            &client,
            &org_row.id,
            Some(&ds_id),
            "dataset_provision",
            0,
            Some(&idem),
            None,
            pg::Provenance::actor(&principal.actor),
        )
        .await
        {
            Ok(j) => j,
            Err(e) => return pg_err(e),
        };
        let _ = pg::audit(
            &client,
            &principal.actor,
            Some(&org),
            Some(&body.name),
            "dataset.provision",
            &json!({ "dataset": body.name, "entity_type": entity_type, "job_id": job.id }),
        )
        .await;
        return (
            StatusCode::ACCEPTED,
            Json(json!({ "name": body.name, "state": "provisioning", "job_id": job.id })),
        )
            .into_response();
    };
    // The same one-source-of-truth rule create_org enforces: config_path and
    // env.ER_CONFIG_PATH must name the same file, because publish writes to the
    // column while every run reads the env.
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
                     name the same file"
                ),
            );
        }
        Some(_) => {}
    }
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    if let Err(r) = refuse_unless_active(&org_row, "create a dataset") {
        return *r;
    }
    let env_json = json!(env_map);
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::create_dataset_byo(
        &client,
        &org_row.id,
        &body.name,
        body.display_name.as_deref(),
        entity_type,
        config_path,
        &env_json,
        body.drop_root.as_deref(),
        body.s3_ingest_bucket.as_deref(),
        body.s3_ingest_prefix.as_deref(),
    )
    .await
    {
        Ok(true) => {
            let _ = pg::audit(
                &client,
                &principal.actor,
                Some(&org),
                Some(&body.name),
                "dataset.create",
                &json!({ "dataset": body.name, "entity_type": entity_type }),
            )
            .await;
            (
                StatusCode::CREATED,
                Json(json!({ "name": body.name, "state": "active" })),
            )
                .into_response()
        }
        Ok(false) => detail_err(
            StatusCode::CONFLICT,
            format!("dataset '{}' already exists in org '{org}'", body.name),
        ),
        Err(e) => env_write_err(e),
    }
}

/// Viewer+: one dataset's detail (`GET /v1/orgs/{org}/datasets/{ds}`).
async fn get_dataset_route(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    let org_row = match org_or_404(&client, &org).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => Json(dataset_json(&row)).into_response(),
        Err(r) => *r,
    }
}

#[derive(Deserialize)]
struct DatasetPatch {
    /// The only mutable display field; `null` clears it. The `name` is the
    /// routing handle and never changes.
    display_name: Option<String>,
}

/// Admin: update a dataset's display fields (`PATCH /v1/orgs/{org}/datasets/{ds}`).
async fn update_dataset(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Json(body): Json<DatasetPatch>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Admin) {
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
    let org_row = match org_or_404(&client, &org).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    if let Err(r) = dataset_or_404(&client, &org_row, &ds).await {
        return *r;
    }
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::set_dataset_display_name(&client, &org_row.id, &ds, body.display_name.as_deref())
        .await
    {
        Ok(()) => match dataset_or_404(&client, &org_row, &ds).await {
            Ok(row) => Json(dataset_json(&row)).into_response(),
            Err(r) => *r,
        },
        Err(e) => pg_err(e),
    }
}

/// Transition a dataset's lifecycle state (the shared body of dataset
/// suspend/resume): scope-safe 404s, 409 if not in `expected`.
async fn transition_dataset(
    principal: &Principal,
    state: &AppState,
    org: &str,
    ds: &str,
    new_state: &str,
    expected: &str,
    not_in_expected: &str,
) -> axum::response::Response {
    if let Err(e) = guard(principal, org, Role::Admin) {
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
    let org_row = match org_or_404(&client, org).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    if let Err(r) = dataset_or_404(&client, &org_row, ds).await {
        return *r;
    }
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::set_dataset_state(&client, &org_row.id, ds, new_state, expected).await {
        Ok(true) => {
            let action = if new_state == "suspended" {
                "dataset.suspend"
            } else {
                "dataset.resume"
            };
            let _ = pg::audit(
                &client,
                &principal.actor,
                Some(org),
                Some(ds),
                action,
                &json!({ "dataset": ds }),
            )
            .await;
            (
                StatusCode::OK,
                Json(json!({ "name": ds, "state": new_state })),
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

/// Admin: suspend an active dataset (`POST /v1/orgs/{org}/datasets/{ds}/suspend`).
async fn suspend_dataset(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
) -> impl IntoResponse {
    let msg = format!("dataset '{ds}' is not active; only active datasets suspend");
    transition_dataset(&principal, &state, &org, &ds, "suspended", "active", &msg).await
}

/// Admin: resume a suspended dataset (`POST /v1/orgs/{org}/datasets/{ds}/resume`).
async fn resume_dataset(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
) -> impl IntoResponse {
    let msg = format!("dataset '{ds}' is not suspended; nothing to resume");
    transition_dataset(&principal, &state, &org, &ds, "active", "suspended", &msg).await
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

/// Admin+: create a draft config version (`POST /v1/orgs/{org}/datasets/{ds}/config/versions`).
/// The YAML is structurally validated and hashed; the version is `max+1`.
async fn create_config_version(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    // History images record the acting principal (er.actor).
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::create_config_version(
        &client,
        &org_row.id,
        Some(d.id.as_str()),
        &body.yaml,
        &hash,
        &principal.actor,
    )
    .await
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
    // The recorded comparator algorithm version, derived from the stored
    // document the `model_weights` way rather than duplicated into a column,
    // so it cannot drift from what the engine loads. Absent in the document
    // reads as 1 — every pre-versioning model is a v1 model; an unparseable
    // document yields null, surfacing the corruption rather than hiding it.
    let comparator_algorithm_version =
        dedupe_core::settings::json::SettingsJson::from_json_str(&m.model_json)
            .ok()
            .map(|s| s.comparator_algorithm_version());
    json!({
        "id": m.id,
        "name": m.name,
        "version": m.version,
        "entity_type": m.entity_type,
        "model_hash": m.model_hash,
        "model_version_label": m.model_version_label,
        "comparator_algorithm_version": comparator_algorithm_version,
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
/// (`POST /v1/orgs/{org}/datasets/{ds}/models:compile`).
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
    Path((org, ds)): Path<(String, String)>,
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    let config = match dataset_active_config(&client, &d).await {
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
        Some(d.id.as_str()),
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
                None,
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

/// Viewer+: the tenant's model versions (`GET /v1/orgs/{org}/datasets/{ds}/models`), newest
/// version of each name first, plus which one is active.
async fn models_index(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    // Scope-correct before anything is served: an unknown dataset 404s.
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    match pg::list_model_versions(&client, &d.id).await {
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

/// Admin+: mint a new model version (`POST /v1/orgs/{org}/datasets/{ds}/models/versions`).
///
/// Versions are immutable — this is the only way a model changes, and it always
/// lands as a `draft`. Activation is a separate, explicit step so that uploading
/// a model can never silently re-cluster a tenant's corpus.
async fn create_model_version(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    // The entity type comes from the org's active config, not the caller: a
    // model scores against one canonical schema and the lake has exactly one.
    let entity_type = match dataset_active_config(&client, &d).await {
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
        Some(d.id.as_str()),
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
                None,
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
/// (`GET /v1/orgs/{org}/datasets/{ds}/models/{name}/versions/{version}`).
async fn get_model_version(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, name, version)): Path<(String, String, String, i64)>,
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
    // Scope-correct before anything is served: an unknown dataset 404s.
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    match pg::model_version(&client, &d.id, &name, version).await {
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
/// (`GET /v1/orgs/{org}/datasets/{ds}/models/{name}/versions/{version}/evaluation`).
///
/// Answers "can I trust this version" with the tenant's own verdicts rather than
/// a number from training. `?threshold=` overrides the org's configured
/// auto-merge threshold, which is the default because that is the cut the product
/// actually acts on.
async fn model_version_evaluation(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, name, version)): Path<(String, String, String, i64)>,
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
    // Scope-correct before anything is served: an unknown dataset 404s.
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    let model = match pg::model_version(&client, &d.id, &name, version).await {
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
    let labels = match er_entities::pair_label::read::all(&client, &d.id).await {
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
/// (`POST /v1/orgs/{org}/datasets/{ds}/models/{name}/versions/{version}/activate`).
///
/// Activation is explicit and recorded, because it decides what the next run
/// scores against — the thing DataGroomr avoids by pinning old models rather
/// than letting a model change re-cluster silently.
async fn activate_model_version(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, name, version)): Path<(String, String, String, i64)>,
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
    // Scope-correct before anything is served: an unknown dataset 404s.
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    let target = match pg::model_version(&client, &d.id, &name, version).await {
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
    if let Err(e) = pg::activate_model_version(&client, &d.id, &target.id).await {
        return pg_err(e);
    }
    let _ = pg::audit(
        &client,
        &principal.actor,
        Some(&org),
        None,
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

/// Viewer+: list an org's config versions, newest first
/// (`GET /v1/orgs/{org}/datasets/{ds}/config/versions`), with no `yaml` body.
async fn list_config_versions(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    // Scope-correct before anything is served: an unknown dataset 404s.
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    match pg::list_config_versions(&client, &d.id).await {
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

/// Viewer+: one config version with its `yaml`
/// (`GET /v1/orgs/{org}/datasets/{ds}/config/versions/{version}`), 404 if absent.
async fn get_config_version(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, version)): Path<(String, String, String)>,
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
    // Scope-correct before anything is served: an unknown dataset 404s.
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    match pg::get_config_version(&client, &d.id, version).await {
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

/// Viewer+: the org's active (published) config (`GET /v1/orgs/{org}/datasets/{ds}/config`),
/// 404 if nothing is published.
async fn active_config(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    // Scope-correct before anything is served: an unknown dataset 404s.
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    match pg::get_config_version(&client, &d.id, active).await {
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

/// Viewer+: the org's nickname lexicon (`GET /v1/orgs/{org}/datasets/{ds}/lexicon`). Returns
/// the packaged default when the org has none yet, so the editor never starts empty.
async fn get_lexicon(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    let pairs: Vec<(String, String)> = match d.env.get("ER_LEXICON_PATH").and_then(Value::as_str) {
        Some(p) if std::path::Path::new(p).exists() => read_lexicon_pairs(p),
        _ => DEFAULT_LEXICON
            .iter()
            .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
            .collect(),
    };
    Json(json!({ "pairs": lexicon_pairs_json(&pairs) })).into_response()
}

/// Steward+: replace the org's nickname lexicon (`PUT /v1/orgs/{org}/datasets/{ds}/lexicon`).
/// Canonicalises + validates each pair, atomically writes the org's
/// `ER_LEXICON_PATH` CSV (deriving + persisting one from `ER_CONFIG_PATH` if
/// unset), then enqueues a `run_all_full` rebuild to apply it.
async fn put_lexicon(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
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
        // Runs read the DATASET env, so the path reaches both stores: the org
        // row (the template future datasets copy) and every existing dataset.
        let env_patch = json!({ "ER_LEXICON_PATH": path });
        if let Err(e) = pg::merge_org_env(&client, &org, &env_patch).await {
            return env_write_err(e);
        }
        if let Err(e) = pg::merge_dataset_envs(&client, &org_row.id, &env_patch).await {
            return env_write_err(e);
        }
    }

    let idem = format!("lexicon-apply:{}", er_ids::new_v7());
    let params = json!({ "skip_ingest": true });
    let apply_job = match pg::enqueue(
        &client,
        &org_row.id,
        Some(d.id.as_str()),
        "run_all_full",
        0,
        Some(&idem),
        Some(&params),
        principal.provenance(),
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
        None,
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

// ─── Ignore-words dictionary editing ────────────────────────────────────────
//
// The spec's second dictionary type (stop words), the synonym lexicon's
// sibling and sharing its whole path: a per-org CSV at `ER_IGNORE_WORDS_PATH`
// (derived from `ER_CONFIG_PATH` when unset), Viewer+ read, Steward+ write,
// canonicalise + validate on write, and a `run_all_full` (skip_ingest) rebuild
// enqueued so the edit takes effect. The `ignore_words` transform reads the
// seeded rows at standardize time.

/// The packaged default returned when an org has no ignore-words file yet —
/// the legal-suffix set `company_name_norm` has always stripped hard-coded,
/// as the editor's starting point (the DEFAULT_LEXICON contract).
const DEFAULT_IGNORE_WORDS: &[(&str, &str)] = &[
    ("co", "legal_suffixes"),
    ("company", "legal_suffixes"),
    ("corp", "legal_suffixes"),
    ("corporation", "legal_suffixes"),
    ("gmbh", "legal_suffixes"),
    ("inc", "legal_suffixes"),
    ("incorporated", "legal_suffixes"),
    ("limited", "legal_suffixes"),
    ("llc", "legal_suffixes"),
    ("llp", "legal_suffixes"),
    ("lp", "legal_suffixes"),
    ("ltd", "legal_suffixes"),
    ("plc", "legal_suffixes"),
];

#[derive(serde::Deserialize)]
struct IgnoreWordRow {
    word: String,
    dictionary: String,
}

#[derive(serde::Deserialize)]
struct IgnoreWordsPut {
    rows: Vec<IgnoreWordRow>,
}

/// Read an ignore-words CSV (`word,dictionary` with a header row) into rows.
/// Missing file → empty.
fn read_ignore_words_rows(path: &str) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            l.split_once(',')
                .map(|(w, d)| (w.trim().to_string(), d.trim().to_string()))
        })
        .collect()
}

fn ignore_words_rows_json(rows: &[(String, String)]) -> Vec<Value> {
    rows.iter()
        .map(|(w, d)| json!({ "word": w, "dictionary": d }))
        .collect()
}

/// Viewer+: the org's ignore-words dictionary
/// (`GET /v1/orgs/{org}/datasets/{ds}/ignore-words`). Returns the packaged
/// default when the org has none yet, so the editor never starts empty.
async fn get_ignore_words(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    let rows: Vec<(String, String)> =
        match d.env.get("ER_IGNORE_WORDS_PATH").and_then(Value::as_str) {
            Some(p) if std::path::Path::new(p).exists() => read_ignore_words_rows(p),
            _ => DEFAULT_IGNORE_WORDS
                .iter()
                .map(|(w, d)| ((*w).to_string(), (*d).to_string()))
                .collect(),
        };
    Json(json!({ "rows": ignore_words_rows_json(&rows) })).into_response()
}

/// Steward+: replace the org's ignore-words dictionary
/// (`PUT /v1/orgs/{org}/datasets/{ds}/ignore-words`). Canonicalises +
/// validates each row, atomically writes the org's `ER_IGNORE_WORDS_PATH` CSV
/// (deriving + persisting one from `ER_CONFIG_PATH` if unset), then enqueues a
/// `run_all_full` rebuild to apply it — the lexicon's apply path.
async fn put_ignore_words(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Json(body): Json<IgnoreWordsPut>,
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
                    "org '{org}' is {}; dictionary edits are accepted only when active",
                    o.state
                ),
            );
        }
        Ok(None) => return detail_err(StatusCode::NOT_FOUND, format!("no org '{org}'")),
        Err(e) => return pg_err(e),
    };
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };

    // Validate + canonicalise. Commas/control chars would corrupt the CSV or
    // the hash separators, and a whitespace-bearing word can never match a
    // whitespace-split token — it is a mistake, said so at write time.
    let mut set: std::collections::BTreeSet<(String, String)> = std::collections::BTreeSet::new();
    for r in &body.rows {
        for v in [&r.word, &r.dictionary] {
            if v.contains(',') || v.chars().any(char::is_control) {
                return detail_err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "words and dictionary names must not contain commas or control characters",
                );
            }
        }
        if r.word.split_whitespace().count() != 1 {
            return detail_err(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!(
                    "{:?} is not a single token; ignore words match whitespace-split tokens",
                    r.word
                ),
            );
        }
        match er_std::ignore_words::canonical_row(&r.word, &r.dictionary) {
            Ok(row) => {
                set.insert(row);
            }
            Err(e) => return detail_err(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()),
        }
    }

    // Resolve the per-org file: the existing ER_IGNORE_WORDS_PATH, else a
    // sibling of ER_CONFIG_PATH — and persist the derivation so runs see it.
    let existing = d
        .env
        .get("ER_IGNORE_WORDS_PATH")
        .and_then(Value::as_str)
        .map(str::to_string);
    let path = match &existing {
        Some(p) => p.clone(),
        None => match org_row.env.get("ER_CONFIG_PATH").and_then(Value::as_str) {
            Some(cfg) => sibling_path(cfg, "ignore_words.csv"),
            None => {
                return detail_err(
                    StatusCode::CONFLICT,
                    "org has no ER_IGNORE_WORDS_PATH, and no ER_CONFIG_PATH to derive one from",
                );
            }
        },
    };

    let mut csv = String::from(
        "word,dictionary
",
    );
    for (w, d) in &set {
        csv.push_str(w);
        csv.push(',');
        csv.push_str(d);
        csv.push('\n');
    }
    if let Err(e) = write_config_file(&path, &csv) {
        return detail_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("ignore-words write: {e}"),
        );
    }
    if existing.is_none() {
        // Runs read the DATASET env, so the path reaches both stores: the org
        // row (the template future datasets copy) and every existing dataset.
        let env_patch = json!({ "ER_IGNORE_WORDS_PATH": path });
        if let Err(e) = pg::merge_org_env(&client, &org, &env_patch).await {
            return env_write_err(e);
        }
        if let Err(e) = pg::merge_dataset_envs(&client, &org_row.id, &env_patch).await {
            return env_write_err(e);
        }
    }

    let idem = format!("ignore-words-apply:{}", er_ids::new_v7());
    let params = json!({ "skip_ingest": true });
    let apply_job = match pg::enqueue(
        &client,
        &org_row.id,
        Some(d.id.as_str()),
        "run_all_full",
        0,
        Some(&idem),
        Some(&params),
        principal.provenance(),
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
        None,
        "ignore_words.update",
        &json!({ "rows": set.len() }),
    )
    .await;

    let rows: Vec<(String, String)> = set.into_iter().collect();
    (
        StatusCode::OK,
        Json(json!({ "rows": ignore_words_rows_json(&rows), "apply_job": apply_job })),
    )
        .into_response()
}

/// Change tiers (`configsvc.py` `classify_tier`), biggest-blast-radius first.
// `comparison_groups` sits with `comparisons`: it changes what a pair scores.
const TIER_C_BLOCKS: [&str; 6] = [
    "comparisons",
    "comparison_groups",
    "standardization",
    "transforms",
    "replacement_dictionaries",
    "sources",
];
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
/// (`POST /v1/orgs/{org}/datasets/{ds}/config/versions/{version}/publish`; the Python
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
    Path((org, ds, version)): Path<(String, String, String)>,
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };

    // The version being published + its re-validated config.
    let detail = match pg::get_config_version(&client, &d.id, version).await {
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
            // Not "references unknown columns": `config_reference_problems`
            // grew past column references — it also rejects a malformed
            // ensemble, a survivorship group mistake, and a `tf: true` with no
            // exact level to scale against. Naming only columns made the
            // refusal misdescribe its own reason, which is worse than a vague
            // message because the reader goes looking for a typo that is not
            // there. The problems themselves each say what is wrong.
            format!(
                "the config cannot be published as written: {}",
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
        match pg::active_models(&client, &d.id).await {
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
            let old_yaml = match pg::get_config_version(&client, &d.id, av).await {
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
    if let Err(e) = pg::publish_config_version(&client, &org, &d.id, version, tier).await {
        return pg_err(e);
    }
    // The published document's `correction_pass.cadence` becomes a real
    // schedule. It had been stored by every tenant config and read by nothing,
    // so the periodic correction pass it describes never ran.
    if let Ok(config) = load_config_str(&detail.yaml)
        && let Err(e) = pg::ensure_correction_schedule(
            &client,
            &org_row.id,
            Some(d.id.as_str()),
            &config.correction_pass.cadence,
        )
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
                Some(d.id.as_str()),
                "train",
                0,
                Some(&train_key),
                None,
                principal.provenance(),
            )
            .await
            {
                Ok(j) => jobs_enqueued.push(j.id),
                Err(e) => return pg_err(e),
            }
            match pg::enqueue(
                &client,
                &org_row.id,
                Some(d.id.as_str()),
                "run_all_full",
                0,
                Some(&rebuild_key),
                Some(&rebuild_params),
                principal.provenance(),
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
            Some(d.id.as_str()),
            "run_all_full",
            0,
            Some(&rebuild_key),
            Some(&rebuild_params),
            principal.provenance(),
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
        None,
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
        Some(d.id.as_str()),
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
        tracing::warn!(er.org = %org, er.version = version, error = %e, "the config.published webhook was not queued");
    }
    // Computed after everything that can fail has already succeeded, so the
    // ordering itself guarantees an advisory cannot cost a tenant a publish.
    let advisories = publish_advisories(&state, &org, &ds, &new_config).await;
    (
        StatusCode::OK,
        Json(json!({
            "org": org,
            "version": version,
            "tier": tier,
            "changed_blocks": changed,
            "jobs_enqueued": jobs_enqueued,
            "published_by": principal.actor,
            // What the latest profile says about the config that just went
            // live — advice, never a refusal, and computed AFTER the publish
            // committed so it cannot affect it even by failing.
            "advisories": advisories,
        })),
    )
        .into_response()
}

/// What the latest standardized profile says about a config's blocking columns.
///
/// Best-effort to the point of being uninterruptible: every failure path
/// returns an empty list. A publish must not depend on the profiler having run,
/// on the lake being openable, or on a read succeeding — the config is already
/// committed by the time this is called, so the only thing an error here could
/// do is turn a successful publish into a failed request.
///
/// Logged rather than silent, because "no advisories" and "the advisory read
/// failed" look identical in the response and only one of them is worth
/// investigating.
async fn publish_advisories(
    state: &AppState,
    org: &str,
    ds: &str,
    config: &er_config::Config,
) -> Vec<String> {
    // The config just published, passed in rather than re-read: advising on a
    // re-fetch would race the write and could describe the previous version.
    let Ok(session) = dataset_lake(state, org, ds).await else {
        return Vec::new();
    };
    let Ok(snapshot) = session.latest_snapshot_version() else {
        return Vec::new();
    };
    match read::profiled_standardized_columns(&session, snapshot) {
        Ok(columns) => config_check::profile_advisories(config, &columns),
        Err(e) => {
            tracing::warn!(er.org = %org, er.dataset = %ds, error = %e, "reading the profile for publish advisories failed");
            Vec::new()
        }
    }
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

/// Steward+: enqueue a job for an org (`POST /v1/orgs/{org}/datasets/{ds}/jobs`).
async fn create_job(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
        let d = match dataset_or_404(&client, &org_row, &ds).await {
            Ok(row) => row,
            Err(r) => return *r,
        };
        if let Err(e) = refuse_unless_active_dataset(&org_row, &d, "jobs") {
            return *e;
        }
        // Resolve the segment NAME against the published config here, so an
        // unknown name is a 422 at submit time naming the segments that do
        // exist — rather than a job that sits in the queue and then fails.
        let mut params = None;
        if let Some(name) = &body.segment {
            let config = match dataset_active_config(&client, &d).await {
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
            Some(d.id.as_str()),
            &body.kind,
            body.priority,
            None,
            params.as_ref(),
            principal.provenance(),
        )
        .await
        {
            Ok(job) => {
                let _ = pg::audit(
                    &client,
                    &principal.actor,
                    Some(&org),
                    None,
                    "job.submit",
                    &json!({ "job_id": job.id, "kind": body.kind, "segment": body.segment }),
                )
                .await;
                (StatusCode::ACCEPTED, Json(json!(job))).into_response()
            }
            Err(e) => pg_err(e),
        };
    }
    let job = enqueue_traced(
        &state,
        &org,
        &body.kind,
        body.priority,
        principal.trace_context.as_deref(),
    );
    (StatusCode::ACCEPTED, Json(json!(job))).into_response()
}

/// Enqueue a `queued` job directly (the body of `create_job`, reusable by the
/// dispatcher and tests). Returns the minted job.
///
/// Untraced: delegates to [`enqueue_traced`] with no trace context, which is
/// what a test or a dispatcher-internal enqueue wants.
pub fn enqueue(state: &AppState, org: &str, kind: &str, priority: i64) -> Job {
    enqueue_traced(state, org, kind, priority, None)
}

/// [`enqueue`], carrying the caller's trace context onto the job so the
/// in-memory store matches the Postgres path. Kept separate rather than widening
/// [`enqueue`] so the existing call sites — all of them tests and the
/// dispatcher, none of which have a request in scope — stay as they are.
pub fn enqueue_traced(
    state: &AppState,
    org: &str,
    kind: &str,
    priority: i64,
    trace_context: Option<&str>,
) -> Job {
    let mut store = state.store.lock().unwrap();
    let job_id = format!("job-{}", store.next);
    store.next += 1;
    let now = chrono::Utc::now();
    let job = Job {
        id: job_id,
        org_id: org.to_string(),
        dataset_id: None,
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
        trace_context: trace_context.map(str::to_string),
        structural_fingerprint: None,
        policy_hash: None,
        image_digest: None,
        clear_number: None,
        owner_epoch: None,
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
/// it is classified and recorded on the row, with its message in
/// `error_detail`, so a test can observe WHY a job failed and not merely that
/// it did. Every class lands in `failed`: retry is per task attempt now (#645),
/// and this helper has no tasks because it has no executor.
pub async fn dispatch_pg<F>(
    client: &tokio_postgres::Client,
    runner: F,
    webhook_http: Option<&reqwest::Client>,
) -> Result<Option<Job>, tokio_postgres::Error>
where
    F: FnOnce(&Job) -> Result<(), String>,
{
    let Some(job) = pg::claim(client, pg::org_job_cap()).await? else {
        return Ok(None);
    };
    pg::mark(client, &job.id, "running").await?;
    let completed = match runner(&job) {
        Ok(()) => {
            pg::mark(client, &job.id, "succeeded").await?;
            Job {
                state: "succeeded".to_string(),
                ..job
            }
        }
        // Classify and record, so the REASON is persisted.
        //
        // This used to `mark(…, "failed")` and DROP the runner's string, which
        // meant a job failed through this helper carried no explanation at all:
        // a test could observe that a bad request failed but never why, and a
        // failed job with no reason is a bug class this repo has already
        // shipped once.
        //
        // The class no longer changes where the row lands — `fail_or_retry`
        // became terminal with the legacy arm (#645). It is recorded because
        // `error_class` is what the UI renders and what `pg_profile.rs` asserts
        // a refused request leaves behind.
        Err(detail) => {
            let class = pg::FailureClass::classify(&detail);
            pg::fail_or_retry(client, &job.id, class, &detail).await?
        }
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

/// Viewer+: list an org's jobs, newest first (`GET /v1/orgs/{org}/datasets/{ds}/jobs`).
async fn list_jobs(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
        let d = match dataset_or_404(&client, &org_row, &ds).await {
            Ok(row) => row,
            Err(r) => return *r,
        };
        // Through `list_jobs_all` scoped to this org: it already takes limit and
        // state and clamps the limit, so the per-org view no longer serializes
        // every job an org has ever had. Filtered to the dataset's own jobs —
        // org-level rows (provision) belong to no dataset's view.
        return match pg::list_jobs_all(&client, Some(&org_row.id), state_filter, limit).await {
            Ok(mut jobs) => {
                jobs.retain(|j| j.dataset_id.as_deref() == Some(d.id.as_str()));
                Json(json!({ "jobs": jobs })).into_response()
            }
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

/// Viewer+: one job by id (`GET /v1/orgs/{org}/datasets/{ds}/jobs/{job_id}`), 404 if absent
/// or not this org's.
async fn get_job(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, job_id)): Path<(String, String, String)>,
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
        // Scope-correct before anything is served: an unknown dataset 404s.
        if let Err(r) = dataset_or_404(&client, &org_row, &ds).await {
            return *r;
        }
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

/// Steward+: cancel a job (`POST /v1/orgs/{org}/datasets/{ds}/jobs/{job_id}/cancel`),
/// idempotent-ish: queued/running -> canceled.
async fn cancel_job(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, job_id)): Path<(String, String, String)>,
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
        // Scope-correct before anything is served: an unknown dataset 404s.
        if let Err(r) = dataset_or_404(&client, &org_row, &ds).await {
            return *r;
        }
        return match pg::get_job(&client, &org_row.id, &job_id).await {
            // Every kind the DAG registry parses is the executor's now that the
            // `ER_EXECUTOR_DAG_KINDS` allowlist is gone (#645), so this is the
            // path essentially every cancel takes. The direct-`mark` arm below
            // survives only for a kind that does not parse, which cannot have
            // been planned and so has no pods to revoke.
            Ok(Some(job)) if dags::JobKind::parse(&job.kind).is_some() => {
                // The executor path (§5.6): a queued job dies directly; a
                // dispatching or running one becomes `canceling` — still
                // inside the one-active-per-org predicate — and the cancel
                // sweep revokes its pods; the webhook fires once, at settle.
                match er_entities::job::update::cancel(&client, &job_id, &org_row.id).await {
                    Ok(rows) => match rows.into_iter().next() {
                        Some(updated) if updated.state == "canceled" => {
                            let _ = webhook::emit_job_event(&client, &updated).await;
                            (StatusCode::OK, Json(json!(updated))).into_response()
                        }
                        Some(updated) => (StatusCode::OK, Json(json!(updated))).into_response(),
                        None => detail_err(
                            StatusCode::CONFLICT,
                            "the job is not cancelable (already terminal)",
                        ),
                    },
                    Err(e) => pg_err(e),
                }
            }
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

/// Steward+: requeue a failed/canceled job
/// (`POST /v1/orgs/{org}/datasets/{ds}/jobs/{job_id}/resume`; the Python `{job_id}:resume`
/// action). 409 if the job isn't in a resumable state. Postgres-only (the
/// in-memory store has no resume path).
///
/// The method and path are kept on ONE doc line deliberately: the MCP tool
/// manifest derives them from these comments, and a path wrapped across two
/// lines parses as a path with a space in it.
async fn resume_job(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, job_id)): Path<(String, String, String)>,
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
    // Scope-correct before anything is served: an unknown dataset 404s.
    if let Err(r) = dataset_or_404(&client, &org_row, &ds).await {
        return *r;
    }
    match pg::resume_job(&client, &org_row.id, &job_id).await {
        Ok(true) => {
            let _ = pg::audit(
                &client,
                &principal.actor,
                Some(&org),
                None,
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

/// Viewer: every try of every task of one job, in topological then try order,
/// archived tries included (`GET /v1/orgs/{org}/datasets/{ds}/jobs/{job_id}/tasks`). The
/// task ledger of batch-orchestration-design §4.2; rows exist once the DAG
/// planner runs (Phase 1 ships the ledger and this read before any executor).
async fn list_job_tasks(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, job_id)): Path<(String, String, String)>,
) -> axum::response::Response {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "the task ledger requires Postgres",
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
    // Scope-correct before anything is served: an unknown dataset 404s.
    if let Err(r) = dataset_or_404(&client, &org_row, &ds).await {
        return *r;
    }
    match er_entities::task_instance::read::by_job(&client, &org_row.id, &job_id).await {
        Ok(tasks) => Json(json!({ "tasks": tasks })).into_response(),
        Err(e) => pg_err(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClearIn {
    /// Extra tries to grant the cleared task, over and above its budget.
    /// Zero re-runs it on its existing budget, which is the ordinary case.
    #[serde(default)]
    retries: Option<i64>,
    /// Re-run at a different resource class — the operator's escalation,
    /// distinct from the automatic OOM escalation (§5.5).
    #[serde(default)]
    resource_class: Option<String>,
}

/// Steward+: clear one task and everything downstream of it, so it re-runs
/// WITHOUT re-running the whole job
/// (`POST /v1/orgs/{org}/datasets/{ds}/jobs/{job_id}/tasks/{task_id}/clear`,
/// §5.4).
///
/// The closure is computed from the COMPILED graph, not from the request: a
/// caller naming one task gets that task plus its transitive downstream,
/// because re-running a stage without re-running what consumed it would leave
/// the lake describing two different runs. That is also why the task id is
/// validated against the job's own DAG — a typo must 404 rather than clear
/// nothing and report success.
///
/// Refused under a canceling job: a clear would hand new work to a job whose
/// pods are being torn down.
async fn clear_job_task(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, job_id, task_id)): Path<(String, String, String, String)>,
    // Optional, so a request with no body and no `content-type` resolves to
    // `None` — this route's ordinary case, "re-run on the task's existing
    // budget". The BFF omits the content-type when it has no body, so the two
    // agree.
    //
    // The TYPED extractor matters beyond ergonomics: `tests/mcp_manifest.rs`
    // derives the body's JSON Schema from it, so taking raw bytes here
    // advertised a bare object and told an MCP caller nothing about the
    // fields below. (That scan finds the first wrapped-body type after the
    // `fn`, so a comment naming one would shadow the real signature — which
    // is why this note spells none of them.)
    body: Option<Json<ClearIn>>,
) -> axum::response::Response {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "the task ledger requires Postgres",
        );
    };
    let input = body.map(|Json(b)| b);
    let mut client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    if let Err(r) = dataset_or_404(&client, &org_row, &ds).await {
        return *r;
    }
    let job = match pg::get_job(&client, &org_row.id, &job_id).await {
        Ok(Some(j)) => j,
        Ok(None) => return detail_err(StatusCode::NOT_FOUND, "no such job"),
        Err(e) => return pg_err(e),
    };
    let Some(kind) = crate::dags::JobKind::parse(&job.kind) else {
        return detail_err(
            StatusCode::CONFLICT,
            "this job's kind has no compiled graph, so it has no task to clear",
        );
    };
    let dag = crate::dags::dag_for(kind);
    let Some(index) = dag.nodes.iter().position(|n| n.task_id == task_id) else {
        return detail_err(
            StatusCode::NOT_FOUND,
            format!("'{task_id}' is not a task of kind '{}'", job.kind),
        );
    };
    // The closure: this task plus its transitive downstream — `er_dag` owns
    // that walk, so the server cannot compute a different answer than the
    // executor would.
    //
    // The probe answers `Present` for every reference, because THIS process
    // cannot open a lake (§3.5 draws that line at the lake, and the control
    // plane is on the wrong side of it). So the clear never drafts in extra
    // upstream work here. That is not a hole: §6.3 step 5 has the POD probe
    // each `LakeRef` through `Prober::snapshot_exists` before it runs, so a
    // genuinely expired upstream surfaces as the cleared task's own
    // `precondition` failure — visible, attributed, and retryable by a wider
    // clear — rather than being silently guessed at from here.
    let plan = er_dag::clear_closure(&dag, &[index], true, |_| {
        er_dag::ReferenceExistence::Present
    });
    let closure: Vec<String> = plan
        .must_run
        .iter()
        .map(|&i| dag.nodes[i].task_id.to_string())
        .collect();
    let retries = input.as_ref().and_then(|b| b.retries).unwrap_or(0);
    let resource_class = input.as_ref().and_then(|b| b.resource_class.as_deref());
    match taskpg::clear(
        &mut client,
        &org_row.id,
        &job_id,
        &closure,
        retries,
        resource_class,
        job.image_digest.as_deref(),
    )
    .await
    {
        Ok(cleared) => (
            StatusCode::ACCEPTED,
            Json(json!({
                // What was cleared, and what each arm of the clear did —
                // three arms, because a live row rotates, a terminal row gets
                // a successor, and a pending row resets in place (§5.4).
                "cleared": closure,
                "restarting": cleared.restarting.len(),
                "rotated": cleared.rotated.len(),
                "reset": cleared.reset.len(),
                "reopened": cleared.job.is_some(),
                // Empty from this process by construction (see the probe
                // above); carried anyway so the shape does not change when
                // the demand resolution gains a lake-side answer.
                "demanded_upstream": plan
                    .demanded_upstream
                    .iter()
                    .map(|&i| dag.nodes[i].task_id)
                    .collect::<Vec<_>>(),
            })),
        )
            .into_response(),
        Err(taskpg::ClearError::Canceling) => detail_err(
            StatusCode::CONFLICT,
            "the job is canceling; a clear under a cancel is refused",
        ),
        Err(taskpg::ClearError::Pg(e)) => pg_err(e),
    }
}

/// Who the attempt channel authenticated as (§6.4).
enum AttemptIdentity {
    /// The per-attempt fallback token: hash match against the row. Pod
    /// identity then comes from the request body, as before.
    Fallback,
    /// The projected ServiceAccount token: Kubernetes proved which pod is
    /// speaking, and `pod_name`/`pod_uid` come from these claims, never from
    /// the body (§6.3 step 3).
    Pod(satoken::PodClaims),
}

impl AttemptIdentity {
    /// The fencing `pod_uid`: the claims' when the platform proved it, the
    /// body's under the fallback token.
    fn pod_uid<'a>(&'a self, from_body: &'a str) -> &'a str {
        match self {
            AttemptIdentity::Pod(claims) => &claims.pod_uid,
            AttemptIdentity::Fallback => from_body,
        }
    }
}

/// Authenticate an attempt-channel call (§6.4): a bearer SHAPED like a JWT is
/// verified offline against the cluster key set and bound to this attempt by
/// the pod name's Job-name prefix — and is never hashed, so a forged token
/// cannot choose the weaker path. Anything else is the per-attempt fallback
/// token `claim_launch` minted, hashed and compared constant-time against the
/// row. Returns the retry lineage, current try first
/// (`task_instance/read/retry_lineage.sql`), and who the caller proved to be.
async fn attempt_authorized(
    client: &tokio_postgres::Client,
    state: &AppState,
    attempt_id: &str,
    headers: &axum::http::HeaderMap,
) -> Result<
    (
        Vec<er_entities::task_instance::TaskInstance>,
        AttemptIdentity,
    ),
    Box<axum::response::Response>,
> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let Some(token) = token else {
        return Err(Box::new(
            unauthorized("missing attempt token").into_response(),
        ));
    };
    let lineage = er_entities::task_instance::read::retry_lineage(client, attempt_id)
        .await
        .map_err(|e| Box::new(pg_err(e)))?;
    if lineage.is_empty() {
        return Err(Box::new(detail_err(
            StatusCode::NOT_FOUND,
            "no such attempt",
        )));
    }
    if satoken::looks_like_jwt(token) {
        let Some(verifier) = state.sa_verifier.as_deref() else {
            return Err(Box::new(
                unauthorized(
                    "a ServiceAccount token was presented, but this server holds no cluster key set",
                )
                .into_response(),
            ));
        };
        let claims = match verifier.verify(token).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(er.attempt_id = %attempt_id, error = %errfmt::chain(&e), "verifying the service-account token failed");
                return Err(Box::new(
                    unauthorized("the ServiceAccount token does not verify").into_response(),
                ));
            }
        };
        // Bind the attempt: the Job name is a pure function of the attempt key
        // (§4.2), a Job's pods are named `<job>-<suffix>`, and the trailing
        // dash keeps `…-l1` from matching an `…-l10` pod.
        //
        // This also excluded the literal `inproc`, the legacy arm's synthetic
        // launch identity. Nothing writes that name now (#645), and the prefix
        // match would have refused it anyway — no Job yields a pod called
        // `inproc-…`.
        let bound = lineage[0]
            .k8s_job_name
            .as_deref()
            .is_some_and(|job| claims.pod_name.starts_with(&format!("{job}-")));
        if !bound {
            return Err(Box::new(
                unauthorized("the token's pod is not this attempt's launch").into_response(),
            ));
        }
        return Ok((lineage, AttemptIdentity::Pod(claims)));
    }
    let presented = pg::hash_secret(token);
    let authorized = lineage[0]
        .token_hash
        .as_deref()
        .is_some_and(|stored| ct_eq(stored.as_bytes(), presented.as_bytes()));
    if !authorized {
        return Err(Box::new(
            unauthorized("the attempt token does not match this attempt").into_response(),
        ));
    }
    Ok((lineage, AttemptIdentity::Fallback))
}

/// The work order for one claimed attempt
/// (`GET /v1/internal/attempts/{attempt_id}`): identity, the pinned input, the
/// job's params, the retry lineage the ledger probe matches on, and the
/// tenant's lake env (§6.3 step 1). Machine-to-machine: per-attempt token, not
/// a principal; the BFF never forwards `/v1/internal/*`.
async fn attempt_work_order(
    State(state): State<AppState>,
    Path(attempt_id): Path<String>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    let Some(dsn) = &state.dsn else {
        return detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "the attempt channel requires Postgres",
        );
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let (lineage, _identity) =
        match attempt_authorized(&client, &state, &attempt_id, &headers).await {
            Ok(l) => l,
            Err(r) => return *r,
        };
    let current = &lineage[0];
    let job = match pg::get_job(&client, &current.org_id, &current.job_id).await {
        Ok(Some(j)) => j,
        Ok(None) => return detail_err(StatusCode::NOT_FOUND, "no job behind this attempt"),
        Err(e) => return pg_err(e),
    };
    // The job's DATASET env, not the org's: each dataset is its own lake, so a
    // pod handed the org-level env for a dataset job would open the wrong one.
    // Same resolver the in-process dispatcher uses (`pg::lake_env_for_job`).
    let (lake_env, drop_root) = match pg::get_org_by_id(&client, &current.org_id).await {
        Ok(Some(org)) => {
            match pg::lake_env_for_job(&client, &org, job.dataset_id.as_deref()).await {
                Ok(Some(pair)) => pair,
                Ok(None) => {
                    return detail_err(StatusCode::CONFLICT, "the job's dataset no longer exists");
                }
                Err(e) => return pg_err(e),
            }
        }
        Ok(None) => (serde_json::Value::Null, None),
        Err(e) => return pg_err(e),
    };
    // The model registry, resolved HERE because the pod holds no
    // control-plane credential (§6.4) and therefore cannot read it. The
    // dataset's own active versions only: each dataset trains and scores
    // against its own corpus. `runctx::RunContext::resolve` turns these plus
    // the lake env and params into everything a stage needs, and the legacy
    // in-process arm calls the same function with the same rows — so the two
    // executors cannot disagree about what a job means.
    let models: Vec<crate::runctx::ActiveModelSpec> =
        match pg::active_models(&client, &current.org_id).await {
            Ok(rows) => rows
                .into_iter()
                .map(|m| crate::runctx::ActiveModelSpec {
                    name: m.name,
                    version: m.version,
                    model_json: m.model_json,
                    model_version_label: m.model_version_label,
                    tf_snapshot_id: m.tf_snapshot_id,
                })
                .collect(),
            Err(e) => return pg_err(e),
        };
    // A training pass learns from the tenant's MATCH labels, which live in
    // Postgres the pod cannot read (§6.4). Filtered here, and only for the one
    // kind that trains: every other order would carry the payload for nothing.
    // A read failure degrades to EM-only rather than failing the attempt —
    // which is exactly what the in-process arm does.
    let labels: Vec<(String, String)> = if job.kind == "train" {
        // The DATASET's labels. `read::all` takes its scope as a bare `&str`
        // named `dataset_id`, so passing the org id compiled and matched
        // nothing — and "no rows" is indistinguishable here from "this tenant
        // has no labels", which is why the m estimate silently fell back to
        // EM-only on every train. Same idiom as everywhere else: the dataset
        // when the job names one, the org id for a pre-dataset row.
        let scope = job.dataset_id.as_deref().unwrap_or(&current.org_id);
        match er_entities::pair_label::read::all(&client, scope).await {
            Ok(rows) => rows
                .into_iter()
                .filter(|l| l.verdict == "match" && l.origin != "llm")
                .map(|l| (l.rec_a_key, l.rec_b_key))
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "reading labels failed; the train falls back to EM");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    // An extract pulls from Salesforce with a credential only Postgres holds
    // (§6.4, the `models`/`labels` precedent). Resolved per attempt rather
    // than frozen at enqueue so a rotated secret takes effect on the next try,
    // and the instance URL is re-checked against the outbound-URL guard HERE —
    // registration already checked it, but DNS can change between the two,
    // which is exactly the webhook delivery path's reasoning.
    let connection: Value = if job.kind == "extract" {
        let request = match crate::extract_params::ExtractRequest::from_params(&job.params) {
            Ok(r) => r,
            Err(m) => return detail_err(StatusCode::CONFLICT, m),
        };
        let Some(dataset_id) = job.dataset_id.as_deref() else {
            return detail_err(StatusCode::CONFLICT, "an extract job must name a dataset");
        };
        let row = match pg::salesforce_connection_credential(
            &client,
            &current.org_id,
            &request.connection_id,
        )
        .await
        {
            Ok(Some(row)) => row,
            Ok(None) => {
                return detail_err(
                    StatusCode::CONFLICT,
                    "the job's salesforce connection no longer exists",
                );
            }
            Err(e) => return pg_err(e),
        };
        if let Err(rejection) = urlguard::check(&row.instance_url, urlguard::Egress::Guarded).await
        {
            return detail_err(
                StatusCode::CONFLICT,
                format!("the connection's instance URL is refused: {rejection}"),
            );
        }
        // The watermark rides beside the credential; a read failure degrades
        // to a full pull, which is correct and at worst re-extracts rows the
        // landing upserts anyway.
        let watermark =
            er_entities::sf_extract_cursor::read::get(&client, dataset_id, &request.source)
                .await
                .ok()
                .and_then(|rows| rows.into_iter().next())
                .and_then(|c| c.watermark);
        json!({
            "instance_url": row.instance_url,
            "client_id": row.client_id,
            "client_secret": row.client_secret,
            "api_version": row.api_version,
            "watermark": watermark,
        })
    } else {
        Value::Null
    };
    let order = json!({
        "org_id": current.org_id,
        "dataset_id": job.dataset_id,
        "job_id": current.job_id,
        // jobs.id == run_id is load-bearing (§4.1); planning writes it.
        "run_id": job.run_id.clone().unwrap_or_else(|| job.id.clone()),
        "kind": job.kind,
        "task_id": current.task_id,
        "try_number": current.try_number,
        "state": current.state,
        "resource_class": current.resource_class,
        "segment": current.segment,
        "trigger_rule": current.trigger_rule,
        "max_tries": current.max_tries,
        "input": current.input,
        "params": job.params,
        "structural_fingerprint": job.structural_fingerprint,
        "lineage": lineage.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
        "lake_env": lake_env,
        // The delivery root is a dataset/org COLUMN, not an env key, so it
        // rides beside the env rather than inside it.
        "drop_root": drop_root,
        "models": models,
        "labels": labels,
        "connection": connection,
    });
    (StatusCode::OK, Json(json!(order))).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttemptStart {
    k8s_job_name: String,
    pod_uid: String,
    #[serde(default)]
    pod_name: Option<String>,
    #[serde(default)]
    node_name: Option<String>,
    #[serde(default)]
    image_digest: Option<String>,
}

/// The pod's first report
/// (`POST /v1/internal/attempts/{attempt_id}/start`): `queued → running`,
/// recording the pod identity. A retry from the same pod returns the running
/// row without resetting its start time. Zero rows means a stale pod — a
/// different launch or a finished attempt — and the caller exits 3 (§6.3 step 3).
async fn attempt_start(
    State(state): State<AppState>,
    Path(attempt_id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<AttemptStart>,
) -> axum::response::Response {
    let Some(dsn) = &state.dsn else {
        return detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "the attempt channel requires Postgres",
        );
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let identity = match attempt_authorized(&client, &state, &attempt_id, &headers).await {
        Ok((_, identity)) => identity,
        Err(r) => return *r,
    };
    // Pod identity comes from the verified claims when the platform proved
    // it, and from the body only under the fallback token (§6.3 step 3).
    let (pod_name, pod_uid) = match &identity {
        AttemptIdentity::Pod(claims) => (Some(claims.pod_name.as_str()), claims.pod_uid.as_str()),
        AttemptIdentity::Fallback => (body.pod_name.as_deref(), body.pod_uid.as_str()),
    };
    match er_entities::task_instance::update::start(
        &client,
        pod_name,
        pod_uid,
        body.node_name.as_deref(),
        body.image_digest.as_deref(),
        &attempt_id,
        &body.k8s_job_name,
    )
    .await
    {
        Ok(rows) => rows.into_iter().next().map_or_else(
            || {
                detail_err(
                    StatusCode::CONFLICT,
                    "stale start: the attempt is not queued under this launch",
                )
            },
            |row| (StatusCode::OK, Json(json!(row))).into_response(),
        ),
        Err(e) => pg_err(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttemptHeartbeat {
    pod_uid: String,
    #[serde(default)]
    counters: Option<serde_json::Value>,
}

/// The 30 s liveness report
/// (`POST /v1/internal/attempts/{attempt_id}/heartbeat`). Zero rows is the
/// FENCE: the pod rolls back its last write-back instead of committing (§6.3
/// step 6).
async fn attempt_heartbeat(
    State(state): State<AppState>,
    Path(attempt_id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<AttemptHeartbeat>,
) -> axum::response::Response {
    let Some(dsn) = &state.dsn else {
        return detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "the attempt channel requires Postgres",
        );
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let identity = match attempt_authorized(&client, &state, &attempt_id, &headers).await {
        Ok((_, identity)) => identity,
        Err(r) => return *r,
    };
    match er_entities::task_instance::update::heartbeat(
        &client,
        body.counters.as_ref(),
        &attempt_id,
        identity.pod_uid(&body.pod_uid),
    )
    .await
    {
        Ok(rows) => rows.into_iter().next().map_or_else(
            || {
                detail_err(
                    StatusCode::CONFLICT,
                    "fenced: the attempt is no longer running as this pod",
                )
            },
            |row| (StatusCode::OK, Json(json!(row))).into_response(),
        ),
        Err(e) => pg_err(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttemptFinish {
    /// `succeeded | skipped | failed | interrupted` — the terminal-record
    /// states of §6.3 step 7.
    state: String,
    pod_uid: String,
    #[serde(default)]
    outcome: Option<String>,
    #[serde(default)]
    output: Option<serde_json::Value>,
    #[serde(default)]
    snapshot_start: Option<i64>,
    #[serde(default)]
    snapshot_end: Option<i64>,
    #[serde(default)]
    counters: Option<serde_json::Value>,
    #[serde(default)]
    exit_code: Option<i64>,
    #[serde(default)]
    error_class: Option<String>,
    #[serde(default)]
    error_detail: Option<String>,
    #[serde(default)]
    termination_reason: Option<String>,
}

/// The pod's terminal report
/// (`POST /v1/internal/attempts/{attempt_id}/finish`): success and skip run
/// `finish.sql`; a failure runs the `fail_or_retry` pair (archive + budgeted
/// successor); `interrupted` records evidence and no verdict — the watch rules
/// once the pod is terminal (§4.5, §5.6). The fused-segment `next` hand-off
/// arrives with the executor in Phase 2.
async fn attempt_finish(
    State(state): State<AppState>,
    Path(attempt_id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<AttemptFinish>,
) -> axum::response::Response {
    let Some(dsn) = &state.dsn else {
        return detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "the attempt channel requires Postgres",
        );
    };
    let mut client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let (lineage, identity) = match attempt_authorized(&client, &state, &attempt_id, &headers).await
    {
        Ok(l) => l,
        Err(r) => return *r,
    };
    let current = lineage.into_iter().next().expect("lineage is non-empty");
    match body.state.as_str() {
        "succeeded" | "skipped" => {
            match er_entities::task_instance::update::finish(
                &client,
                body.state == "skipped",
                body.outcome.as_deref().unwrap_or("done"),
                body.output.as_ref(),
                body.snapshot_start,
                body.snapshot_end,
                body.counters.as_ref(),
                body.exit_code,
                &attempt_id,
                identity.pod_uid(&body.pod_uid),
            )
            .await
            {
                Ok(rows) => rows.into_iter().next().map_or_else(
                    || {
                        detail_err(
                            StatusCode::CONFLICT,
                            "stale finish: the attempt is no longer running as this pod",
                        )
                    },
                    |row| {
                        ops::task_completed(&row);
                        // The pod holds no control-plane credential (§6.4), so
                        // a reconcile_schema task's tenant-state report lands
                        // here, from its output document.
                        // BOTH receipts that reconcile a lake. `lake_init`
                        // emits `lake_initialized`, and filtering only on
                        // `schema_reconciled` meant a provisioned tenant never
                        // got a `tenant_schema_states` row — the one table the
                        // deploy report and the drift check consult so they do
                        // not have to open a lake.
                        let schema_doc = body
                            .output
                            .as_ref()
                            .filter(|o| {
                                o["plan"] == "schema_reconciled" || o["plan"] == "lake_initialized"
                            })
                            .map(|o| o["doc"].clone());
                        if let Some(doc) = schema_doc
                            && let Some(dsn) = state.dsn.clone()
                        {
                            let org_id = row.org_id.clone();
                            let dataset_id = row.dataset_id.clone();
                            let status = doc["status"].as_str().unwrap_or("errored").to_string();
                            let hash = doc["catalog_hash"].as_str().unwrap_or("").to_string();
                            let detail = doc["detail"].as_str().unwrap_or("").to_string();
                            tokio::spawn(async move {
                                let Ok(client) = pg::connect(&dsn).await else {
                                    return;
                                };
                                let _ = crate::schema::record_tenant_state(
                                    &client,
                                    &org_id,
                                    dataset_id.as_deref(),
                                    &status,
                                    &hash,
                                    Some(&detail),
                                )
                                .await;
                            });
                        }
                        (StatusCode::OK, Json(json!(row))).into_response()
                    },
                ),
                Err(e) => pg_err(e),
            }
        }
        "failed" => {
            let class = body.error_class.as_deref().unwrap_or("internal");
            let retryable = matches!(
                class,
                "transient_io" | "lock_conflict" | "infrastructure" | "resources"
            );
            let backoff = if current.kind == "control" {
                taskpg::CONTROL_BACKOFF
            } else {
                taskpg::POD_BACKOFF
            };
            let failure = taskpg::TaskFailure {
                terminal_state: if class == "timeout" {
                    "timed_out"
                } else {
                    "failed"
                },
                retryable,
                escalate: class == "resources",
                exit_code: body.exit_code,
                termination_reason: body.termination_reason.as_deref(),
                error_class: Some(class),
                error_detail: body.error_detail.as_deref(),
                k8s_job_name: current.k8s_job_name.as_deref(),
                backoff,
            };
            // A FAILED schema reconcile still has a tenant position to record.
            //
            // The success branch above writes `tenant_schema_states` from the
            // task's output doc, and a failing task publishes no output — so a
            // breaking lake left the tenant's recorded position at whatever the
            // last good sweep said, which is `current`. The deploy report and
            // the drift check read that table precisely so they do not have to
            // open a lake, and they were being told a breaking tenant was fine.
            //
            // The legacy arm recorded this outcome "whatever it was", in as many
            // words; the task path recorded it only on success. `status` comes
            // from the failure text the same way `schema::tenant_state` derives
            // it, so ERR_SCHEMA_BREAKING reads `breaking` and anything else
            // `errored`.
            if matches!(current.task_id.as_str(), "reconcile_schema" | "lake_init")
                && let Some(dsn) = state.dsn.clone()
            {
                let detail = body.error_detail.clone().unwrap_or_default();
                let status = if detail.contains("ERR_SCHEMA_BREAKING") {
                    "breaking"
                } else {
                    "errored"
                };
                let org_id = current.org_id.clone();
                let dataset_id = current.dataset_id.clone();
                tokio::spawn(async move {
                    let Ok(client) = pg::connect(&dsn).await else {
                        return;
                    };
                    let _ = crate::schema::record_tenant_state(
                        &client,
                        &org_id,
                        dataset_id.as_deref(),
                        status,
                        "",
                        Some(&detail),
                    )
                    .await;
                });
            }
            match taskpg::fail_or_retry_task(&mut client, &attempt_id, &failure).await {
                Ok((Some(archived), successor)) => (
                    StatusCode::OK,
                    Json(json!({ "archived": archived, "retry": successor })),
                )
                    .into_response(),
                Ok((None, _)) => detail_err(
                    StatusCode::CONFLICT,
                    "stale failure report: the attempt is not live under this launch",
                ),
                Err(e) => pg_err(e),
            }
        }
        "interrupted" => {
            match er_entities::task_instance::update::note_interruption(
                &client,
                body.termination_reason.as_deref().unwrap_or("sigterm"),
                body.counters.as_ref(),
                &attempt_id,
                identity.pod_uid(&body.pod_uid),
            )
            .await
            {
                Ok(rows) => rows.into_iter().next().map_or_else(
                    || {
                        detail_err(
                            StatusCode::CONFLICT,
                            "stale interruption: the attempt is not live as this pod",
                        )
                    },
                    |row| (StatusCode::OK, Json(json!(row))).into_response(),
                ),
                Err(e) => pg_err(e),
            }
        }
        other => detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("unknown terminal state '{other}'"),
        ),
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
    let filters = pg::AuditFilters {
        action: params.get("action").map(String::as_str),
        actor: params.get("actor").map(String::as_str),
        since: params.get("since").map(String::as_str),
        until: params.get("until").map(String::as_str),
    };
    let before_id = params.get("before_id").and_then(|s| s.parse::<i64>().ok());
    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(100)
        .clamp(1, 500);
    pg::audit_rows(client, org, &filters, before_id, limit)
        .await
        .map_err(|e| Box::new(pg_err(e)))
}

/// The `format=csv` arm of the audit feeds: the same page the JSON arm would
/// list, as text/csv. Split from the handlers so both share one export; the
/// JSON arm stays inline in each handler because the envelope gate classifies
/// handlers by reading their own `Json(...)` call.
fn audit_csv_response(rows: &[Value]) -> axum::response::Response {
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/csv; charset=utf-8")],
        read::audit_csv(rows),
    )
        .into_response()
}

/// Operator: the cross-tenant audit feed (`GET /v1/audit`), optional
/// `org`/`action`/`actor`/`since`/`until`/`before_id` filters; `format=csv`
/// exports the same page.
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
        Ok(rows) => {
            if params.get("format").map(String::as_str) == Some("csv") {
                return audit_csv_response(&rows);
            }
            Json(json!(rows)).into_response()
        }
        Err(r) => *r,
    }
}

/// Admin+: an org's audit feed (`GET /v1/orgs/{org}/audit`), optional
/// `action`/`actor`/`since`/`until`/`before_id` filters; `format=csv` exports
/// the same page.
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
        Ok(rows) => {
            if params.get("format").map(String::as_str) == Some("csv") {
                return audit_csv_response(&rows);
            }
            Json(json!(rows)).into_response()
        }
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
    let org_row = match org_or_404(&client, &org).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    let patch = Value::Object(updates);
    if let Err(e) = pg::merge_org_env(&client, &org, &patch).await {
        return env_write_err(e);
    }
    // Resource knobs are org-wide and runs read the DATASET env: fan the patch
    // out to every dataset, exactly as the lexicon path does.
    if let Err(e) = pg::merge_dataset_envs(&client, &org_row.id, &patch).await {
        return env_write_err(e);
    }
    match pg::get_org(&client, &org).await {
        Ok(Some(o)) => Json(resources_of(&o.env)).into_response(),
        Ok(None) => detail_err(StatusCode::NOT_FOUND, format!("no org '{org}'")),
        Err(e) => pg_err(e),
    }
}

/// Resolve `{org}/{ds}` to the dataset's row, or a ready error response (404
/// for either level — scope-safe both ways — 501 without Postgres, 500 on a
/// read failure). Every dataset-scoped lake handler starts here.
async fn dataset_for_lake(
    state: &AppState,
    org: &str,
    ds: &str,
) -> Result<pg::DatasetDetail, Box<axum::response::Response>> {
    let Some(dsn) = &state.dsn else {
        return Err(Box::new(detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "read API requires Postgres",
        )));
    };
    let client = connect_pg(dsn).await.map_err(|r| Box::new(*r))?;
    let org_row = match pg::get_org(&client, org).await {
        Ok(Some(o)) => o,
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
    match pg::get_dataset(&client, &org_row.id, ds).await {
        Ok(Some(d)) => Ok(d),
        Ok(None) => Err(Box::new(detail_err(
            StatusCode::NOT_FOUND,
            format!("no dataset '{ds}' in org '{org}'"),
        ))),
        Err(e) => Err(Box::new(detail_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("db: {e}"),
        ))),
    }
}

/// Open one dataset's lake read-only. Returns the attached session, or a ready
/// error response (404 no org/dataset, 501 no Postgres backend, 503 lake
/// unavailable).
async fn dataset_lake(
    state: &AppState,
    org: &str,
    ds: &str,
) -> Result<dedupe_core::Session, Box<axum::response::Response>> {
    let d = dataset_for_lake(state, org, ds).await?;
    read::open_org_lake(&d.env).map_err(|e| {
        Box::new(detail_err(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("lake unavailable: {e}"),
        ))
    })
}

/// Like [`dataset_lake`], but also resolves the dataset's auto-merge threshold
/// from its config (for the run ledger's per-run auto-merge count). The
/// threshold is `None` when the dataset has no readable config.
async fn dataset_lake_auto_merge(
    state: &AppState,
    org: &str,
    ds: &str,
) -> Result<(dedupe_core::Session, Option<f64>), Box<axum::response::Response>> {
    let d = dataset_for_lake(state, org, ds).await?;
    let auto_merge = auto_merge_from_env(&d.env);
    let session = read::open_org_lake(&d.env).map_err(|e| {
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

/// Like [`dataset_lake`], but also resolves the dataset's entity type (its own
/// column — no config re-parse). The golden/entity reads dispatch on it so an
/// account dataset reads its `acct_golden_*` projection.
async fn dataset_lake_golden(
    state: &AppState,
    org: &str,
    ds: &str,
) -> Result<(dedupe_core::Session, er_config::EntityType), Box<axum::response::Response>> {
    let d = dataset_for_lake(state, org, ds).await?;
    let entity_type = match d.entity_type.as_str() {
        "account" => er_config::EntityType::Account,
        _ => er_config::EntityType::Person,
    };
    let session = read::open_org_lake(&d.env).map_err(|e| {
        Box::new(detail_err(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("lake unavailable: {e}"),
        ))
    })?;
    Ok((session, entity_type))
}

/// Viewer+: a keyset page of the org's golden (resolved) records
/// (`GET /v1/orgs/{org}/datasets/{ds}/golden-records`). Query: `q` (contains-search), `cursor`
/// (opaque), `limit` (<=200). Reads the org's DuckLake at a pinned snapshot.
async fn golden_records_list(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let (session, entity_type) = match dataset_lake_golden(&state, &org, &ds).await {
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

/// The optional point in history a golden read is taken at (O12c).
///
/// Absent means the latest snapshot, which is what every caller wanted before
/// field-value rollback existed. A pinned one is how the UI shows what a field
/// USED to be: DuckLake keeps the old versions, so the history needs no new
/// table — only the ability to ask for a version other than the newest.
#[derive(Debug, serde::Deserialize)]
struct SnapshotQuery {
    snapshot: Option<i64>,
}

/// Viewer+: one resolved entity with members/lineage/events
/// (`GET /v1/orgs/{org}/datasets/{ds}/golden-records/{entity_id}`), 404 if absent.
///
/// `?snapshot=N` reads the entity AS IT WAS at lake version N. Out-of-range
/// versions are refused by the lake itself rather than clamped: silently
/// serving the newest data for a pinned historical read would show a reader
/// today's value under yesterday's heading.
async fn golden_record_detail(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, entity_id)): Path<(String, String, String)>,
    Query(query): Query<SnapshotQuery>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let (session, entity_type) = match dataset_lake_golden(&state, &org, &ds).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let snapshot = match query.snapshot {
        Some(pinned) => pinned,
        None => match read::latest_snapshot(&session) {
            Ok(s) => s,
            Err(e) => return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}")),
        },
    };
    match read::entity_detail(&session, entity_type, snapshot, &entity_id) {
        Ok(Some(detail)) => Json(detail).into_response(),
        Ok(None) => detail_err(StatusCode::NOT_FOUND, format!("no entity '{entity_id}'")),
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
}

/// Viewer+: dashboard counters over the org's lake (`GET /v1/orgs/{org}/datasets/{ds}/metrics`).
async fn org_metrics(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match dataset_lake(&state, &org, &ds).await {
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
/// (`GET /v1/orgs/{org}/datasets/{ds}/data-quality?stale_days=N`). Read-only analytics;
/// `stale_days` (default 365) is a presentation parameter, never config.
async fn org_data_quality(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    let session = match dataset_lake(&state, &org, &ds).await {
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
/// (`GET /v1/orgs/{org}/datasets/{ds}/duplicates`). Keyset page; query `cursor`,
/// `limit`, and the filter bar's parameters: `member` (a record key whose group
/// is returned), `min_members`/`max_members`, `min_confidence`/`max_confidence`
/// (0–100 display scale), `matched_by` (model version), `matched_on`
/// (comparison column), `verified` (true/false), `tag`/`exclude_tag`.
async fn duplicates_list(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let (session, entity_type) = match dataset_lake_golden(&state, &org, &ds).await {
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
    let number = |key: &str| params.get(key).and_then(|v| v.trim().parse::<i64>().ok());
    let text = |key: &str| {
        params
            .get(key)
            .map(String::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let mut filters = read::DuplicateFilters {
        // The clusters an over-merge hides in are the LARGE ones, and a page
        // ordered by entity id will not surface them on its own.
        min_members: number("min_members"),
        max_members: number("max_members"),
        min_confidence: number("min_confidence"),
        max_confidence: number("max_confidence"),
        model_version: text("matched_by"),
        matched_on: text("matched_on"),
        verified: params.get("verified").and_then(|v| match v.trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }),
        ..read::DuplicateFilters::default()
    };
    // An inverted band matches nothing, and "nothing" looks like an empty
    // dataset — name the typo instead (the reviews handler does the same for
    // its probability band).
    if let (Some(lo), Some(hi)) = (filters.min_members, filters.max_members)
        && lo > hi
    {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("min_members {lo} is above max_members {hi}; the band is empty"),
        );
    }
    if let (Some(lo), Some(hi)) = (filters.min_confidence, filters.max_confidence)
        && lo > hi
    {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("min_confidence {lo} is above max_confidence {hi}; the band is empty"),
        );
    }
    // The member-record search: resolve the key to its entity and pin the
    // list to it, so the group payload is assembled identically on both
    // paths. An unknown key is an empty result, not an error.
    if let Some(member) = text("member") {
        match read::entity_of_record_key(&session, snapshot, &member) {
            Ok(Some(entity)) => filters.only_entity = Some(entity),
            Ok(None) => {
                return Json(json!({
                    "items": [],
                    "snapshot": snapshot,
                    "next_cursor": null,
                }))
                .into_response();
            }
            Err(e) => {
                return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}"));
            }
        }
    }
    // Tag include/exclude: the taggings live in the control-plane Postgres,
    // so these are the one filter dimension that leaves the lake. Resolved
    // to entity-id sets here; the read filters by membership.
    let (tag, exclude_tag) = (text("tag"), text("exclude_tag"));
    if tag.is_some() || exclude_tag.is_some() {
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
        let dataset = match dataset_or_404(&client, &org_row, &ds).await {
            Ok(d) => d,
            Err(r) => return *r,
        };
        for (name, slot) in [
            (&tag, &mut filters.tagged),
            (&exclude_tag, &mut filters.excluded),
        ] {
            let Some(name) = name else { continue };
            let ids = match pg::tag_by_name(&client, &org_row.id, name).await {
                // An unknown tag names nothing: an include matches no group,
                // an exclude forbids none — both honest answers.
                Ok(None) => std::collections::HashSet::new(),
                Ok(Some(tag)) => {
                    // One past the cap, so "at the cap" and "truncated at the
                    // cap" are distinguishable — silent truncation would make
                    // an over-broad tag quietly filter by only its first
                    // 10,000 subjects.
                    match pg::subjects_for_tag(
                        &client,
                        &dataset.id,
                        &tag.id,
                        Some("entity"),
                        10_001,
                    )
                    .await
                    {
                        Ok(rows) if rows.len() > 10_000 => {
                            return detail_err(
                                StatusCode::UNPROCESSABLE_ENTITY,
                                format!(
                                    "tag {name:?} marks more than 10,000 entities — too broad to filter by"
                                ),
                            );
                        }
                        Ok(rows) => rows.into_iter().map(|t| t.subject_id).collect(),
                        Err(e) => return pg_err(e),
                    }
                }
                Err(e) => return pg_err(e),
            };
            *slot = Some(ids);
        }
    }
    match read::duplicates(
        &session,
        entity_type,
        snapshot,
        after.as_deref(),
        &filters,
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
/// (`GET /v1/orgs/{org}/datasets/{ds}/insights/sources`). Query `run_id`; defaults to the
/// newest snapshot's run.
///
/// The corpus-wide rate says a tenant has a problem; this says which feed it
/// came from, which is the difference between knowing and being able to act.
async fn insight_sources(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match dataset_lake(&state, &org, &ds).await {
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

/// Viewer+: the resolution metrics over time (`GET /v1/orgs/{org}/datasets/{ds}/insights`).
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
    Path((org, ds)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match dataset_lake(&state, &org, &ds).await {
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

/// Viewer+: the corpus's data-quality picture (`GET /v1/orgs/{org}/datasets/{ds}/quality`).
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
    Path((org, ds)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match dataset_lake(&state, &org, &ds).await {
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

/// Viewer+: a corpus stage's profile
/// (`GET /v1/orgs/{org}/datasets/{ds}/profile`). Query: `subject`
/// (`raw|standardized|golden`, default standardized), `source` (required for
/// `raw`), `tier` (default 1).
///
/// Read-only and viewer-level for the same reason the pre-merge report is:
/// describing a corpus writes nothing. The caveat is the five fields that echo
/// values as stored rather than describing them — `top_values`, `lex_min`,
/// `lex_max`, `num_min`, `num_max` — which are gated to steward+. A viewer gets
/// every statistic with those nulled, and `top_values` keeps its counts and
/// shares, which is what a skew finding actually needs.
///
/// Steward is the bar because a steward can already read the records
/// themselves. It is not a bar a viewer can route around: `GET …/records` takes
/// explicit keys (capped at 50) and will not enumerate, so a column's most
/// frequent values are not otherwise reachable by someone who cannot already
/// name the rows they want.
///
/// A corpus nobody has profiled answers `{"profiled": false}` rather than 404:
/// the dataset exists and the question is fair, the answer is just "not yet".
async fn dataset_profile(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let subject = params
        .get("subject")
        .map_or("standardized", String::as_str)
        .to_string();
    let request = match crate::profile_params::ProfileRequest::from_params(&json!({
        "subject": subject,
        "source": params.get("source"),
        "tier": params.get("tier").and_then(|t| t.parse::<i64>().ok()).unwrap_or(1),
    })) {
        Ok(r) => r,
        // The same parser the writer uses, so a subject this endpoint accepts
        // is one a job could actually run.
        Err(message) => return detail_err(StatusCode::UNPROCESSABLE_ENTITY, message),
    };
    let session = match dataset_lake(&state, &org, &ds).await {
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
    let source_system = request
        .source
        .clone()
        .unwrap_or_else(|| er_cli::profile::NO_SOURCE.to_string());
    let include_values = guard(&principal, &org, Role::Steward).is_ok();
    match read::profile(
        &session,
        snapshot,
        request.subject.as_str(),
        &source_system,
        request.tier,
        include_values,
    ) {
        Ok(body) => Json(body).into_response(),
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
}

/// Steward+: profile a corpus stage
/// (`POST /v1/orgs/{org}/datasets/{ds}/profile`). Body: the same `subject`,
/// `source`, `tier`, `exact_distinct`, `sample` and `sample_seed` a `profile`
/// job takes. Returns 202 and the job id.
///
/// Steward rather than viewer because this one WRITES — a profile is persisted
/// so it can be compared against the next one — and it takes the dataset's
/// writer slot like any other job.
///
/// The params are validated HERE, through the same parser the dispatcher uses,
/// so a bad request is a 422 naming the problem rather than a job that sits in
/// the queue and then fails. That is the rule the segment resolution in
/// `create_job` already follows.
async fn dataset_profile_run(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    if let Err(message) = crate::profile_params::ProfileRequest::from_params(&body) {
        return detail_err(StatusCode::UNPROCESSABLE_ENTITY, message);
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "profiling requires Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    if let Err(e) = refuse_unless_active_dataset(&org_row, &d, "profiling") {
        return *e;
    }
    match pg::enqueue(
        &client,
        &org_row.id,
        Some(d.id.as_str()),
        "profile",
        0,
        None,
        Some(&body),
        principal.provenance(),
    )
    .await
    {
        Ok(job) => (
            StatusCode::ACCEPTED,
            Json(json!({ "job": { "id": job.id, "kind": job.kind, "state": job.state } })),
        )
            .into_response(),
        Err(e) => pg_err(e),
    }
}

/// Viewer+: active pairwise match scores (`GET /v1/orgs/{org}/datasets/{ds}/match-scores`).
/// Keyset page; query `cursor`, `limit`, `band_low`/`band_high` (probability
/// band). The band names must match what the BFF forwards
/// (`src/app/api/orgs/[org]/match-scores/route.ts`): a rename on one side only
/// drops the filter silently, which is what `min`/`max` here used to do.
async fn match_scores_list(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match dataset_lake(&state, &org, &ds).await {
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

/// Viewer+: what to label next (`GET /v1/orgs/{org}/datasets/{ds}/labeling/next`).
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
    Path((org, ds)): Path<(String, String)>,
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    let config = match dataset_active_config(&client, &d).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let labels = match er_entities::pair_label::read::all(&client, &d.id).await {
        Ok(l) => l,
        Err(e) => return pg_err(e),
    };
    let coverage = label_coverage(&labels);
    let labelled: std::collections::BTreeSet<(String, String)> = labels
        .iter()
        .map(|l| (l.rec_a_key.clone(), l.rec_b_key.clone()))
        .collect();

    let session = match dataset_lake(&state, &org, &ds).await {
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

/// Steward+: record a verdict on a pair (`POST /v1/orgs/{org}/datasets/{ds}/labels`).
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
    Path((org, ds)): Path<(String, String)>,
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    match record_label(
        &client,
        &org_row.id,
        Some(d.id.as_str()),
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
                None,
                "label.record",
                &json!({ "pair": [a, b], "verdict": body.verdict }),
            )
            .await;
            (StatusCode::CREATED, Json(json!(label))).into_response()
        }
        Err(e) => pg_err(e),
    }
}

/// Viewer+: the labels a tenant has recorded (`GET /v1/orgs/{org}/datasets/{ds}/labels`),
/// newest first, filterable by `verdict` and `origin`.
async fn labels_list(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    // Scope-correct before anything is served: an unknown dataset 404s.
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    let limit = read::parse_limit(params.get("limit").map(String::as_str));
    match er_entities::pair_label::read::page(
        &client,
        &d.id,
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
    dataset_id: Option<&str>,
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
        dataset_id,
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
/// `GET /v1/orgs/{org}/datasets/{ds}/match-preview`). Runs the committed model over the current
/// corpus and returns the pairs it WOULD score, bucketed by the config
/// thresholds, with a sample carrying the evidence waterfall. Writes nothing —
/// no `match_scores` / `review_queue` / golden, no determinism-contract artifact.
/// `?limit=` caps the sample. The matcher is heavy + holds a !Send session, so it
/// runs on a blocking thread.
async fn match_preview(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    let org_row = match pg::get_org(&client, &org).await {
        Ok(Some(o)) if o.state == "active" => o,
        Ok(Some(o)) => {
            return detail_err(
                StatusCode::CONFLICT,
                format!("org '{org}' is {}; preview needs an active org", o.state),
            );
        }
        Ok(None) => return detail_err(StatusCode::NOT_FOUND, format!("no org '{org}'")),
        Err(e) => return pg_err(e),
    };
    let env = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(d) => d.env,
        Err(r) => return *r,
    };
    let limit = read::parse_limit(params.get("limit").map(String::as_str))
        .try_into()
        .unwrap_or(50usize);

    match er_obs::spawn_blocking_in_span(move || match_preview_blocking(&env, limit)).await {
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
/// (`POST /v1/orgs/{org}/datasets/{ds}/resolve`).
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
    Path((org, ds)): Path<(String, String)>,
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
    let org_row = match pg::get_org(&client, &org).await {
        Ok(Some(o)) if o.state == "active" => o,
        Ok(Some(o)) => {
            return detail_err(
                StatusCode::CONFLICT,
                format!("org '{org}' is {}; resolve needs an active org", o.state),
            );
        }
        Ok(None) => return detail_err(StatusCode::NOT_FOUND, format!("no org '{org}'")),
        Err(e) => return pg_err(e),
    };
    let env = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(d) => d.env,
        Err(r) => return *r,
    };
    let limit = body.limit.unwrap_or(10).clamp(1, 100);
    let source = body.source.clone();
    let record = body.record.clone();

    match er_obs::spawn_blocking_in_span(move || resolve_blocking(&env, &source, &record, limit))
        .await
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
    let ignore_words = s("ER_IGNORE_WORDS_PATH")
        .map(load_lexicon_file)
        .unwrap_or_default();
    let session = crate::schema::open_lake_env(env)?;
    let model = er_cli::MatchModel {
        settings_json: &settings_json,
        tf_csv_path: std::path::Path::new(tf_path),
        model_version: s("ER_MODEL_VERSION").unwrap_or("v0001"),
        tf_snapshot_id: s("ER_TF_SNAPSHOT_ID").unwrap_or("tf"),
    };
    let result = er_cli::resolve_record(
        &session,
        &config,
        &model,
        source,
        record,
        er_cli::Dictionaries {
            lexicon: &lexicon,
            ignore_words: &ignore_words,
        },
        limit,
    )
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
/// (`GET /v1/orgs/{org}/datasets/{ds}/blocking-estimate`). Query: `largest` (how many
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
    Path((org, ds)): Path<(String, String)>,
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
    let org_row = match pg::get_org(&client, &org).await {
        Ok(Some(o)) if o.state == "active" => o,
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
    let env = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(d) => d.env,
        Err(r) => return *r,
    };
    let largest = params
        .get("largest")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(5)
        .min(50);

    match er_obs::spawn_blocking_in_span(move || blocking_estimate_blocking(&env, largest)).await {
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

    let session = crate::schema::open_lake_env(env)?;
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
    let session = crate::schema::open_lake_env(env)?;
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
                "confidence": dedupe_core::confidence::Confidence::from_probability(p.match_probability).display(),
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
/// (`GET /v1/orgs/{org}/datasets/{ds}/records?key=…`, `key` repeats, capped at 50). Not
/// paginated — a bounded lookup. Returns `{ items, snapshot }`.
async fn records_by_key(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match dataset_lake(&state, &org, &ds).await {
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

/// Viewer+: the open-review queue (`GET /v1/orgs/{org}/datasets/{ds}/reviews`).
async fn reviews_list(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match dataset_lake(&state, &org, &ds).await {
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

/// Viewer+: one review by id (`GET /v1/orgs/{org}/datasets/{ds}/reviews/{review_id}`), 404 if absent.
async fn review_detail(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, review_id)): Path<(String, String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match dataset_lake(&state, &org, &ds).await {
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

/// Viewer+: the review merge dialog's group
/// (`GET /v1/orgs/{org}/datasets/{ds}/reviews/{review_id}/merge-group`) — the
/// union of BOTH sides' entity memberships with values, so the dialog edits the
/// merge that would actually land, not just the pair the review names.
async fn review_merge_group(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, review_id)): Path<(String, String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match dataset_lake(&state, &org, &ds).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let snapshot = match read::latest_snapshot(&session) {
        Ok(s) => s,
        Err(e) => return detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake: {e}")),
    };
    match read::merge_group(&session, snapshot, &review_id) {
        Ok(Some(read::MergeGroup::Group { items, total })) => {
            Json(json!({ "items": items, "total": total, "snapshot": snapshot })).into_response()
        }
        Ok(Some(read::MergeGroup::NoPair)) => detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "this review is about an entity rather than a pair of records",
        ),
        Ok(None) => detail_err(StatusCode::NOT_FOUND, format!("no review '{review_id}'")),
        Err(e) => detail_err(StatusCode::SERVICE_UNAVAILABLE, format!("lake read: {e}")),
    }
}

/// Viewer+: the assertion ledger (`GET /v1/orgs/{org}/datasets/{ds}/assertions`), active-only
/// unless `?include_retracted=true`.
async fn assertions_list(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match dataset_lake(&state, &org, &ds).await {
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
/// A DATASET's active published config, parsed — [`org_active_config`]'s
/// dataset-scoped twin, reading the version pointer off the dataset row.
pub(crate) async fn dataset_active_config(
    client: &tokio_postgres::Client,
    d: &pg::DatasetDetail,
) -> Result<er_config::Config, Box<axum::response::Response>> {
    let Some(active) = d.active_config_version else {
        return Err(Box::new(detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no published config; publish a config version first",
        )));
    };
    let detail = match pg::get_config_version(client, &d.id, active).await {
        Ok(Some(v)) => v,
        Ok(None) => {
            return Err(Box::new(detail_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("active config version {active} is missing"),
            )));
        }
        Err(e) => return Err(Box::new(pg_err(e))),
    };
    load_config_str(&detail.yaml).map_err(|e| {
        Box::new(detail_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("active config does not load: {}", errfmt::chain(&e)),
        ))
    })
}

/// The active config of an org's DEFAULT dataset.
///
/// A compatibility shim for the callers that are still org-driven — the S3
/// poller sweeps orgs, and `source_priority` is asked for by org — now that
/// config versions are addressed by dataset. It resolves the default dataset
/// rather than taking one, which is the same answer those callers got before
/// datasets existed. The clean-break phase removes it along with the org's
/// own `active_config_version` mirror; everything dataset-scoped already
/// calls [`dataset_active_config`].
pub(crate) async fn org_active_config(
    client: &tokio_postgres::Client,
    org_row: &pg::OrgDetail,
) -> Result<er_config::Config, Box<axum::response::Response>> {
    let Some(active) = org_row.active_config_version else {
        return Err(Box::new(detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no published config; publish a config version first",
        )));
    };
    let Ok(Some(ds_id)) = pg::default_dataset_id(client, &org_row.id).await else {
        return Err(Box::new(detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "the org has no default dataset to read a config from",
        )));
    };
    let yaml = match pg::get_config_version(client, &ds_id, active).await {
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
pub(crate) async fn delivery_preconditions(
    client: &tokio_postgres::Client,
    org: &str,
    ds: &str,
    source: &str,
) -> Result<(pg::OrgDetail, pg::DatasetDetail, er_config::Config, String), Box<Refusal>> {
    let org_row = org_or_404(client, org).await?;
    if org_row.state != "active" {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            format!(
                "org '{org}' is {}; imports are accepted only when active",
                org_row.state
            ),
        ));
    }
    let d = dataset_or_404(client, &org_row, ds).await?;
    if d.state != "active" {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            format!(
                "dataset '{ds}' is {}; imports are accepted only when active",
                d.state
            ),
        ));
    }
    let config = dataset_active_config(client, &d).await?;
    if !config.sources.contains_key(source) {
        let known: Vec<&str> = config.sources.keys().map(String::as_str).collect();
        return Err(Refusal::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "source '{source}' is not in the active config (known: {}); \
                 publish a config version containing sources.{source} first",
                known.join(", ")
            ),
        ));
    }
    let Some(drop_root) = d.drop_root.clone() else {
        return Err(Refusal::new(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("dataset '{ds}' has no drop_root to land a delivery into"),
        ));
    };
    // The source is a key of the active config, so it cannot traverse; assert it
    // anyway, because this value becomes a path segment.
    if !source
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(Refusal::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("source '{source}' is not a valid path segment"),
        ));
    }
    if let Err(e) = tokio::fs::create_dir_all(&drop_root).await {
        return Err(Refusal::new(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("drop_root {drop_root} is not writable: {e}"),
        ));
    }
    Ok((org_row, d, config, drop_root))
}

/// Publish immutable bytes and their manifest before enqueueing their consumer.
pub(crate) struct StagedDelivery<'a> {
    pub(crate) org: &'a str,
    pub(crate) org_id: &'a str,
    pub(crate) source: &'a str,
    pub(crate) delivery_id: &'a str,
    pub(crate) dataset_id: &'a str,
    pub(crate) drop_root: &'a str,
    pub(crate) spec: &'a er_config::SourceSpec,
    pub(crate) bytes: u64,
    /// The audit action naming how it arrived (`import.upload` / `import.push`
    /// / `import.s3`).
    pub(crate) action: &'a str,
}

pub(crate) async fn finish_delivery(
    client: &tokio_postgres::Client,
    actor: &str,
    trace_context: Option<&str>,
    d: &StagedDelivery<'_>,
) -> axum::response::Response {
    let StagedDelivery {
        org,
        org_id,
        source,
        delivery_id,
        dataset_id,
        drop_root,
        spec,
        bytes,
        action,
    } = *d;
    let root = std::path::PathBuf::from(drop_root);
    let dataset = dataset_id.to_owned();
    let source_name = source.to_owned();
    let id = delivery_id.to_owned();
    let spec = spec.clone();
    let pin = match er_obs::spawn_blocking_in_span(move || {
        delivery::publish(&root, &dataset, &source_name, &id, &spec)
    })
    .await
    {
        Ok(Ok(pin)) => pin,
        result => {
            tracing::warn!(error = ?result, "delivery publication failed; retained files permit recovery");
            return detail_err(
                StatusCode::SERVICE_UNAVAILABLE,
                "cannot publish the delivery",
            );
        }
    };
    // One stable key per accepted delivery, never per source. An enqueue retry
    // re-finds this consumer without collapsing distinct accepted deliveries.
    let idem = format!("delivery:{delivery_id}");
    let ds = Some(dataset_id);
    let job = match pg::enqueue(
        client,
        org_id,
        ds,
        "run_all_incremental",
        0,
        Some(&idem),
        Some(&pin.params()),
        pg::Provenance {
            created_by: Some(actor),
            trace_context,
        },
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
        None,
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
        ds,
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
        tracing::warn!(er.org = %org, er.source = %source, error = %e, "the import.received webhook was not queued");
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

/// Steward+: push records as JSON (`POST /v1/orgs/{org}/datasets/{ds}/records:ingest`).
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
    Path((org, ds)): Path<(String, String)>,
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
    let (org_row, d, config, drop_root) =
        match delivery_preconditions(&client, &org, &ds, &body.source).await {
            Ok(t) => t,
            Err(r) => return r.into_response(),
        };
    let spec = &config.sources[&body.source];

    // The columns this source declares, in a stable order: its record id, its
    // updated-at stamp, then every mapped column. Taken from the config rather
    // than from the records so a missing value is an empty cell, not a shift —
    // and factored with the Salesforce extract, which must render the same
    // order or its deliveries would shift columns silently.
    let columns = crate::extract_params::source_csv_columns(spec);

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
        principal.trace_context.as_deref(),
        &StagedDelivery {
            org: &org,
            org_id: &org_row.id,
            source: &body.source,
            delivery_id: &delivery_id,
            dataset_id: &d.id,
            drop_root: &drop_root,
            spec,
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
/// (`POST /v1/orgs/{org}/datasets/{ds}/imports?source=<source>`), then enqueue the run that
/// picks it up.
///
/// This is the HTTP half of ingest. Bytes are staged under a private name,
/// then `finish_delivery` publishes an immutable file and manifest before it
/// queues the exact consumer. Concurrent deliveries cannot replace its input.
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
    Path((org, ds)): Path<(String, String)>,
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
    let (org_row, d, config, drop_root) =
        match delivery_preconditions(&client, &org, &ds, source).await {
            Ok(t) => t,
            Err(r) => return r.into_response(),
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
        principal.trace_context.as_deref(),
        &StagedDelivery {
            org: &org,
            org_id: &org_row.id,
            source,
            delivery_id: &delivery_id,
            dataset_id: &d.id,
            drop_root: &drop_root,
            spec: &config.sources[source],
            bytes: bytes_written,
            action: "import.upload",
        },
    )
    .await
}

/// Viewer+: ingest-batch receipts (`GET /v1/orgs/{org}/datasets/{ds}/imports`), optional
/// `?source` filter.
async fn imports_list(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match dataset_lake(&state, &org, &ds).await {
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
/// (`GET /v1/orgs/{org}/datasets/{ds}/imports/recent`), newest first, a bare array — the
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
    Path((org, ds)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match dataset_lake(&state, &org, &ds).await {
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

/// Viewer+: this tenant's auto-ingest state
/// (`GET /v1/orgs/{org}/datasets/{ds}/s3-ingest`) — the watched prefix, and per source what
/// was last read from it.
///
/// Exists because auto-ingest is the one delivery path with no request to look
/// at. An upload has an HTTP response and a push has a status code; a dropped
/// object has only whatever the dispatcher logged, which a tenant cannot read.
/// Without this, "I put the file in the bucket" and "nothing happened" are
/// indistinguishable from the outside — the same gap the delivery-receipts
/// panel closed for uploads.
///
/// A Postgres read, not a lake read: the cursor is control-plane state. So this
/// answers even for a tenant whose lake will not open, which is exactly when
/// someone is asking.
async fn s3_ingest_state(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(
            StatusCode::NOT_IMPLEMENTED,
            "auto-ingest state requires Postgres",
        );
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(o) => o,
        Err(r) => return *r,
    };
    // Scope-correct before anything is served: an unknown dataset 404s.
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    let sources = match er_entities::s3_ingest_cursor::read::by_org(&client, &d.id).await {
        Ok(rows) => rows,
        Err(e) => return pg_err(e),
    };
    Json(json!({
        "bucket": org_row.s3_ingest_bucket,
        "prefix": org_row.s3_ingest_prefix,
        "sources": sources.iter().map(|c| json!({
            "source": c.source,
            "object_key": c.object_key,
            "size_bytes": c.size_bytes,
            "delivery_id": c.delivery_id,
            // The entity's own names. A wire rename to
            // `first_ingested_at`/`last_ingested_at` read better in isolation
            // and was dropped: it is a second vocabulary for one column, the
            // generated `S3IngestCursor` binding already describes what each
            // means, and `mock-fabrication.test.ts` is right that a field no
            // entity declares has to be accounted for rather than assumed.
            "created_at": pg::ts(&c.created_at),
            "updated_at": pg::ts(&c.updated_at),
        })).collect::<Vec<_>>(),
    }))
    .into_response()
}

/// Viewer+: the 20 most recent runs (`GET /v1/orgs/{org}/datasets/{ds}/runs`), a bare array.
async fn runs_list(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let (session, auto_merge) = match dataset_lake_auto_merge(&state, &org, &ds).await {
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
/// (`GET /v1/orgs/{org}/datasets/{ds}/runs/{run_id}/entity-events`). Keyset page; query
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
    Path((org, ds, run_id)): Path<(String, String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match dataset_lake(&state, &org, &ds).await {
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
/// (`GET /v1/orgs/{org}/datasets/{ds}/merge-plans`). Query: `policy` (master-election, default
/// `most_attributes`), `since` (ISO8601), `limit`, `format` (json|csv).
async fn merge_plans(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    let session = match dataset_lake(&state, &org, &ds).await {
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
pub const SCHEDULABLE_KINDS: [&str; 8] = [
    "run_all_full",
    "run_all_incremental",
    "correct",
    "train",
    "lake_maintain",
    // O5c. Harmless to schedule on a tenant with no `approval.tag`: the arm
    // reads the config, finds no gate, and does nothing.
    "apply_approved",
    // A profile is worth scheduling because its value is comparative: one
    // profile describes a corpus, a series of them shows it drifting. Params
    // default to the standardized corpus at tier 1 with exact distinct counts,
    // so a bare `{}` schedule is the useful one.
    "profile",
    // The Salesforce poll (spec 1.24: incremental by default). A schedule is
    // the whole point of an incremental extract — its params carry the
    // connection, object and source, validated at schedule-create time by the
    // same parser the executor uses.
    "extract",
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

/// Admin+: create an API-sourced schedule (`POST /v1/orgs/{org}/datasets/{ds}/schedules`).
async fn create_schedule(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    let params = body.params.unwrap_or_else(|| json!({}));
    // An extract schedule's params are its whole meaning — a bare `{}` would
    // enqueue jobs that fail at planning, forever, on a cron. Validated by the
    // same parser the executor uses, the `dataset_profile_run` rule.
    if body.kind == "extract"
        && let Err(message) = crate::extract_params::ExtractRequest::from_params(&params)
    {
        return detail_err(StatusCode::UNPROCESSABLE_ENTITY, message);
    }
    // History images record the acting principal (er.actor).
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::create_schedule(
        &client,
        &org_row.id,
        Some(d.id.as_str()),
        &body.kind,
        &body.cron,
        &params,
    )
    .await
    {
        Ok(schedule_id) => (
            StatusCode::CREATED,
            Json(json!({ "id": schedule_id, "kind": body.kind, "cron": body.cron })),
        )
            .into_response(),
        Err(e) => pg_err(e),
    }
}

/// Viewer+: list an org's schedules (`GET /v1/orgs/{org}/datasets/{ds}/schedules`).
async fn schedules_index(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    // Scope-correct before anything is served: an unknown dataset 404s.
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    match pg::list_schedules(&client, &d.id).await {
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

/// Admin+: enable/disable a schedule
/// (`PATCH /v1/orgs/{org}/datasets/{ds}/schedules/{schedule_id}`).
/// Config/system-owned rows refuse (409).
async fn update_schedule(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, schedule_id)): Path<(String, String, String)>,
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
    // Scope-correct before anything is served: an unknown dataset 404s.
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    // History images record the acting principal (er.actor).
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::set_schedule_enabled(&client, &d.id, &schedule_id, body.enabled).await {
        Ok(true) => Json(json!({ "id": schedule_id, "enabled": body.enabled })).into_response(),
        Ok(false) => detail_err(
            StatusCode::CONFLICT,
            "schedule is unknown, belongs to another org, or is config-owned",
        ),
        Err(e) => pg_err(e),
    }
}

/// Admin+: delete a schedule (`DELETE /v1/orgs/{org}/datasets/{ds}/schedules/{schedule_id}`).
async fn delete_schedule(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, schedule_id)): Path<(String, String, String)>,
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
    // Scope-correct before anything is served: an unknown dataset 404s.
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    // History images record the acting principal (er.actor).
    let _ = pg::set_actor(&client, &principal.actor).await;
    match pg::delete_schedule(&client, &d.id, &schedule_id).await {
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
/// (`GET /v1/orgs/{org}/datasets/{ds}/segments`).
///
/// The same discovery reason as the webhook catalog: a run names a segment, so
/// the names have to be obtainable. Returning the predicate alongside each name
/// is deliberate — it is the tenant's own published config, and an operator
/// choosing how to split a long run needs to see what each slice covers.
async fn segments_catalog(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    let config = match dataset_active_config(&client, &d).await {
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
    match pg::create_webhook(&client, &org_row.id, None, &body.url, &events, &secret).await {
        Ok(webhook_id) => {
            let _ = pg::audit(
                &client,
                &principal.actor,
                Some(&org),
                None,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TagIn {
    name: String,
    #[serde(default)]
    description: Option<String>,
    /// The minimum role allowed to apply the tag — `steward` (the route
    /// default) or `admin`. Omitted means steward.
    #[serde(default)]
    apply_min_role: Option<String>,
}

/// Viewer+: the tenant's tag vocabulary, or the tags on one subject
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
    // Case-insensitive because the unique index is case-sensitive: a
    // case-sensitive check would let `Merge` squat on the system's `merge`.
    if pg::RESERVED_TAG_NAMES
        .iter()
        .any(|r| r.eq_ignore_ascii_case(name))
    {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "{name:?} is reserved for a tag the platform writes itself; reserved names are {:?}",
                pg::RESERVED_TAG_NAMES
            ),
        );
    }
    let apply_min_role = match body.apply_min_role.as_deref() {
        None => None,
        Some(s @ ("steward" | "admin")) => Some(s.to_string()),
        // `viewer` is below the route's floor and `operator` would lock the
        // admins out of their own vocabulary — neither is a real choice.
        Some(other) => {
            return detail_err(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("apply_min_role must be steward or admin, got {other:?}"),
            );
        }
    };
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
    match pg::create_tag(
        &client,
        &org_row.id,
        name,
        description,
        apply_min_role.as_deref(),
        &principal.actor,
    )
    .await
    {
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
    match pg::tag_by_id(&client, &org_row.id, &tag_id).await {
        // A system tag names a mark the platform writes; retiring it by hand
        // would leave the action that set it writing a tag that reads as
        // gone. 422 rather than 403: no role may do this, so it is not a
        // role denial.
        Ok(Some(tag)) if tag.kind == "system" => {
            return detail_err(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!(
                    "{:?} is a system tag; the platform retires its own",
                    tag.name
                ),
            );
        }
        Ok(Some(_)) => {}
        Ok(None) => return detail_err(StatusCode::NOT_FOUND, "no such tag"),
        Err(e) => return pg_err(e),
    }
    match pg::retire_tag(&client, &org_row.id, &tag_id).await {
        // A JSON body rather than 204, matching `delete_webhook`: the BFF's only
        // forwarding helper parses the response as JSON, so a 204 reaches it as
        // `SyntaxError: Unexpected end of JSON input` — the upstream write
        // succeeds and the UI shows an error. No control-plane mutation here
        // answers 204, and this is why.
        Ok(true) => Json(json!({ "retired": true })).into_response(),
        Ok(false) => detail_err(StatusCode::NOT_FOUND, "no such tag"),
        Err(e) => pg_err(e),
    }
}

/// The link kinds the API accepts, mirroring `entity_link.kind`'s accepted
/// values. Checked here rather than left to the column constraint so a typo
/// comes back as a 422 naming the alternatives instead of a 500 naming a
/// Postgres check.
pub const LINK_KINDS: [&str; 3] = ["household", "company", "related"];

#[derive(Deserialize)]
struct EntityLinkIn {
    parent_entity_id: String,
    child_entity_id: String,
    kind: String,
    #[serde(default)]
    note: Option<String>,
}

/// Steward+: link two entities WITHOUT merging them
/// (`POST /v1/orgs/{org}/datasets/{ds}/entity-links`), or correct an existing link of the
/// same kind.
///
/// The product has one answer to "these two records belong together" — merge
/// them — and that answer is wrong for a husband and wife at one address, or a
/// subsidiary and its parent. Merging loses the distinction and no later run
/// recovers it; recording nothing loses the relationship.
///
/// Steward, not admin: this is the same judgement a steward already makes when
/// they resolve a review, and it is strictly LESS destructive than the merge
/// they can already perform. Gating it higher would mean the cheapest way to
/// record "these are related" is to merge them, which is the outcome this
/// exists to avoid.
async fn create_entity_link(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Json(body): Json<EntityLinkIn>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    if !LINK_KINDS.contains(&body.kind.as_str()) {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("kind must be one of {LINK_KINDS:?}"),
        );
    }
    let (parent, child) = (body.parent_entity_id.trim(), body.child_entity_id.trim());
    if parent.is_empty() || child.is_empty() {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "parent_entity_id and child_entity_id are required",
        );
    }
    // A self-link is meaningless and would render as an entity in its own
    // household — refused here because nothing downstream would notice.
    if parent == child {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "an entity cannot be linked to itself",
        );
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "entity links require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    let _ = pg::set_actor(&client, &principal.actor).await;
    match er_entities::entity_link::create::upsert(
        &client,
        &er_ids::new_v7(),
        &org_row.id,
        Some(d.id.as_str()),
        parent,
        child,
        &body.kind,
        // `manual` is the only origin this route mints. A caller-supplied
        // origin would let a steward's link claim to be a model's suggestion,
        // or the reverse — and the whole point of the column is that a surface
        // can tell a judgement from a guess.
        "manual",
        body.note.as_deref(),
        &principal.actor,
    )
    .await
    {
        Ok(rows) => {
            let _ = pg::audit(
                &client,
                &principal.actor,
                Some(&org),
                None,
                "entity.link",
                &json!({ "parent": parent, "child": child, "kind": body.kind }),
            )
            .await;
            (StatusCode::CREATED, Json(json!(rows.first()))).into_response()
        }
        Err(e) => pg_err(e),
    }
}

/// Viewer+: every link touching one entity, from either side
/// (`GET /v1/orgs/{org}/datasets/{ds}/entities/{entity_id}/links`).
async fn entity_links(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, entity_id)): Path<(String, String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "entity links require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    // Scope-correct before anything is served: an unknown dataset 404s.
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    match er_entities::entity_link::read::for_entity(&client, &d.id, &entity_id).await {
        Ok(rows) => Json(json!({ "items": rows })).into_response(),
        Err(e) => pg_err(e),
    }
}

/// Steward+: remove a link (`DELETE /v1/orgs/{org}/datasets/{ds}/entity-links/{link_id}`).
///
/// Scoped by org in the statement itself, so a link id learned elsewhere
/// cannot delete another tenant's row even if this handler's org check were
/// ever loosened.
async fn delete_entity_link(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, link_id)): Path<(String, String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "entity links require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    // Scope-correct before anything is served: an unknown dataset 404s.
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    let _ = pg::set_actor(&client, &principal.actor).await;
    match er_entities::entity_link::delete::by_id(&client, &d.id, &link_id).await {
        Ok(0) => detail_err(StatusCode::NOT_FOUND, "no such link"),
        Ok(_) => {
            let _ = pg::audit(
                &client,
                &principal.actor,
                Some(&org),
                None,
                "entity.unlink",
                &json!({ "link_id": link_id }),
            )
            .await;
            // A JSON body, not 204: every other DELETE in this router answers
            // with one, and the BFF's `forward` parses the response — a bare
            // 204 reaches it as an empty body and throws "Unexpected end of
            // JSON input", which is a BFF crash rather than a failed delete.
            Json(json!({ "deleted": link_id })).into_response()
        }
        Err(e) => pg_err(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TagApplyIn {
    subject_type: String,
    subject_id: String,
}

/// Steward+: apply or remove a tag
/// (`POST /v1/orgs/{org}/datasets/{ds}/tags/{tag_id}/apply` and `.../unapply`).
///
/// Two routes onto one handler because the bodies and the checks are identical
/// and only the verb differs; `apply` is idempotent at the schema, so a repeat
/// is 204 rather than a duplicate row.
async fn apply_tag(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, tag_id)): Path<(String, String, String)>,
    Json(body): Json<TagApplyIn>,
) -> impl IntoResponse {
    tag_application(principal, state, org, ds, tag_id, body, true).await
}

/// Steward+: remove a tag from a subject
/// (`POST /v1/orgs/{org}/datasets/{ds}/tags/{tag_id}/unapply`).
async fn unapply_tag(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, tag_id)): Path<(String, String, String)>,
    Json(body): Json<TagApplyIn>,
) -> impl IntoResponse {
    tag_application(principal, state, org, ds, tag_id, body, false).await
}

async fn tag_application(
    principal: Principal,
    state: AppState,
    org: String,
    ds: String,
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    // The tag's own guards, read from the row before writing. Existence first
    // (404) — which leaks nothing, since Viewer+ can list the whole vocabulary
    // — then the system/written-by-hand guard, then the per-tag role floor as
    // a 403 (the house rule: same-org, insufficient role).
    match pg::tag_by_id(&client, &org_row.id, &tag_id).await {
        Ok(Some(tag)) => {
            // A system tag is the platform's own mark; a human applying one by
            // hand forges provenance (a steward-marked `unmatch` reads
            // identically to the matcher's). The operator credential — what
            // the platform's writers authenticate as — is exempt. 422 rather
            // than 403: this is not a role floor, no role may cross it.
            if tag.kind == "system" && principal.role != Role::Operator {
                return detail_err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!(
                        "{:?} is written by the platform, not applied by hand",
                        tag.name
                    ),
                );
            }
            match tag.apply_min_role.as_deref().map(Role::parse) {
                Some(Some(min)) => {
                    if principal.role != Role::Operator && principal.role < min {
                        return detail_err(
                            StatusCode::FORBIDDEN,
                            format!("tag {:?} requires {} role to apply", tag.name, min.as_str()),
                        );
                    }
                }
                // A floor the server cannot read is not a floor that lifts:
                // reachable only by writing the row directly (the API
                // validates, fresh stores CHECK it), so fail CLOSED with a
                // 500 — the data is broken, not the caller.
                Some(None) => {
                    return detail_err(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("tag {:?} has an unreadable apply_min_role", tag.name),
                    );
                }
                None => {}
            }
        }
        // Unknown or retired: the writes below answer that (404), on apply as
        // "no such tag" and on unapply as "not applied to that subject".
        Ok(None) => {}
        Err(e) => return pg_err(e),
    }
    let outcome = if apply {
        pg::apply_tag(
            &client,
            &org_row.id,
            Some(&d.id),
            &tag_id,
            &body.subject_type,
            &body.subject_id,
            &principal.actor,
        )
        .await
    } else {
        pg::unapply_tag(
            &client,
            &d.id,
            &tag_id,
            &body.subject_type,
            &body.subject_id,
        )
        .await
    };
    match outcome {
        // A JSON body rather than 204, for the reason `retire_tag` records.
        Ok(true) => {
            if apply {
                Json(json!({ "applied": true })).into_response()
            } else {
                Json(json!({ "removed": true })).into_response()
            }
        }
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

/// Viewer+: what carries a tag (`GET /v1/orgs/{org}/datasets/{ds}/tags/{tag_id}/subjects`).
async fn tag_subjects(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, tag_id)): Path<(String, String, String)>,
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
    // Scope-correct before anything is served: an unknown dataset 404s.
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    let limit = read::parse_limit(params.get("limit").map(String::as_str));
    match pg::subjects_for_tag(
        &client,
        &d.id,
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

/// Viewer+: every tagging of one subject kind in one dataset
/// (`GET /v1/orgs/{org}/datasets/{ds}/taggings?subject_type=entity`).
///
/// The bulk read an export prequeries: joining tags onto an exported page
/// per row would be one query per row, so the caller takes the whole set once
/// and joins client-side. Capped, with the cap+1 probe distinguishing "at the
/// cap" from "truncated at the cap" — truncation is reported in the envelope
/// (`truncated: true`) rather than exporting a subset that reads as complete.
async fn taggings_index(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    // The kind is required rather than defaulted: the only bulk caller today
    // wants entities, and a default would make a caller who forgot the param
    // believe it had everything.
    let Some(subject_type) = params.get("subject_type").map(String::as_str) else {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "subject_type is required, one of {:?}",
                pg::TAG_SUBJECT_KINDS
            ),
        );
    };
    if !pg::is_tag_subject(subject_type) {
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    /// Past this a tenant has outgrown the prequery shape and the export wants
    /// a taggings join in the lake read instead — a real design conversation,
    /// not a silent subset.
    const TAGGINGS_CAP: i64 = 100_000;
    match pg::taggings_for_dataset(&client, &d.id, subject_type, TAGGINGS_CAP + 1).await {
        Ok(rows) => {
            let truncated = rows.len() as i64 > TAGGINGS_CAP;
            let rows = if truncated {
                &rows[..TAGGINGS_CAP as usize]
            } else {
                &rows[..]
            };
            Json(json!({ "taggings": rows, "truncated": truncated })).into_response()
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
                    None,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SalesforceConnectionIn {
    /// The operator's label, unique per org.
    name: String,
    /// The Salesforce My Domain base URL, https.
    instance_url: String,
    /// The connected app's consumer key.
    client_id: String,
    /// The connected app's consumer secret.
    client_secret: String,
    /// The OAuth flow; only `client_credentials` today, which is also the default.
    #[serde(default)]
    auth_kind: Option<String>,
    /// The REST API version segment, e.g. `v62.0`; defaulted when omitted.
    #[serde(default)]
    api_version: Option<String>,
}

/// The REST version a connection gets when the caller does not pin one.
const DEFAULT_SF_API_VERSION: &str = "v62.0";

/// Admin+: register a Salesforce connection (`POST /v1/orgs/{org}/connections`).
/// The client secret is stored for the extract job's client-credentials flow
/// and is never returned by any endpoint — not even this one's 201, which
/// echoes only the non-secret half (the caller already holds the secret; an
/// echo would be one more place it lands).
async fn create_salesforce_connection(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<SalesforceConnectionIn>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Admin) {
        return e.into_response();
    }
    for (field, value) in [
        ("name", &body.name),
        ("instance_url", &body.instance_url),
        ("client_id", &body.client_id),
        ("client_secret", &body.client_secret),
    ] {
        if value.trim().is_empty() {
            return detail_err(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("{field} is required"),
            );
        }
    }
    let auth_kind = body.auth_kind.as_deref().unwrap_or("client_credentials");
    if auth_kind != "client_credentials" {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "auth_kind {auth_kind:?} is not supported; the extract job is headless, so \
                 only client_credentials is"
            ),
        );
    }
    let api_version = body
        .api_version
        .as_deref()
        .unwrap_or(DEFAULT_SF_API_VERSION);
    if !api_version
        .strip_prefix('v')
        .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit() || c == '.'))
    {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("api_version {api_version:?} is not a version segment like v62.0"),
        );
    }
    // The outbound-URL guard, exactly as webhook registration runs it: the
    // extract pod will POST this tenant-supplied URL with credentials attached,
    // so an internal or metadata address must be refused before it reaches the
    // table. The work-order resolver re-checks, because DNS can change.
    if let Err(rejection) = urlguard::check(&body.instance_url, urlguard::Egress::Guarded).await {
        return detail_err(StatusCode::UNPROCESSABLE_ENTITY, rejection.to_string());
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "connections require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    match pg::list_salesforce_connections(&client, &org_row.id).await {
        Ok(rows) if rows.iter().any(|c| c.name == body.name) => {
            return detail_err(
                StatusCode::CONFLICT,
                format!("a connection named {:?} already exists", body.name),
            );
        }
        Ok(_) => {}
        Err(e) => return pg_err(e),
    }
    match pg::create_salesforce_connection(
        &client,
        &org_row.id,
        &body.name,
        &body.instance_url,
        auth_kind,
        &body.client_id,
        &body.client_secret,
        api_version,
    )
    .await
    {
        Ok(connection_id) => {
            // The audit detail names the connection and its URL — never the
            // secret, the key.issue rule.
            let _ = pg::audit(
                &client,
                &principal.actor,
                Some(&org),
                None,
                "connection.create",
                &json!({
                    "connection_id": connection_id,
                    "name": body.name,
                    "instance_url": body.instance_url,
                    "auth_kind": auth_kind,
                }),
            )
            .await;
            (
                StatusCode::CREATED,
                Json(json!({
                    "id": connection_id,
                    "name": body.name,
                    "instance_url": body.instance_url,
                    "auth_kind": auth_kind,
                    "api_version": api_version,
                    "active": true,
                })),
            )
                .into_response()
        }
        Err(e) => pg_err(e),
    }
}

/// Viewer+: list an org's active Salesforce connections
/// (`GET /v1/orgs/{org}/connections`); the client secret is never listed.
async fn salesforce_connections_index(
    principal: Principal,
    State(state): State<AppState>,
    Path(org): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "connections require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    match pg::list_salesforce_connections(&client, &org_row.id).await {
        Ok(rows) => {
            let out: Vec<Value> = rows
                .iter()
                .map(|c| {
                    json!({
                        "id": c.id,
                        "name": c.name,
                        "instance_url": c.instance_url,
                        "auth_kind": c.auth_kind,
                        "api_version": c.api_version,
                        "active": c.active,
                    })
                })
                .collect();
            Json(json!(out)).into_response()
        }
        Err(e) => pg_err(e),
    }
}

/// Admin+: retire a Salesforce connection
/// (`DELETE /v1/orgs/{org}/connections/{connection_id}`). Soft, like webhooks:
/// cursors and past jobs reference the id, and a queued extract against it
/// fails with "no longer exists" rather than resolving a dangling credential.
async fn delete_salesforce_connection(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, connection_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Admin) {
        return e.into_response();
    }
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "connections require Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    match pg::delete_salesforce_connection(&client, &org_row.id, &connection_id).await {
        Ok(deleted) => {
            if deleted {
                let _ = pg::audit(
                    &client,
                    &principal.actor,
                    Some(&org),
                    None,
                    "connection.delete",
                    &json!({ "connection_id": connection_id }),
                )
                .await;
            }
            Json(json!({ "deleted": deleted })).into_response()
        }
        Err(e) => pg_err(e),
    }
}

/// Steward+: pull an sObject from Salesforce into this dataset
/// (`POST /v1/orgs/{org}/datasets/{ds}/extract`). Body: `connection_id`,
/// `object`, `source`, optional `mode` (`incremental` default | `full`),
/// `where`, `exclude_converted_leads`. Returns 202 and the job id.
///
/// Validated HERE through the same parser the executor uses, so a bad request
/// is a 422 naming the problem rather than a job that sits in the queue and
/// then fails — the `dataset_profile_run` rule. The connection is also
/// resolved now: a dangling id is the caller's typo, not the dispatcher's
/// discovery.
async fn dataset_extract_run(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    let request = match crate::extract_params::ExtractRequest::from_params(&body) {
        Ok(r) => r,
        Err(message) => return detail_err(StatusCode::UNPROCESSABLE_ENTITY, message),
    };
    let Some(dsn) = &state.dsn else {
        return detail_err(StatusCode::NOT_IMPLEMENTED, "extraction requires Postgres");
    };
    let client = match connect_pg(dsn).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let org_row = match org_or_404(&client, &org).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    if let Err(e) = refuse_unless_active_dataset(&org_row, &d, "extraction") {
        return *e;
    }
    match pg::salesforce_connection_credential(&client, &org_row.id, &request.connection_id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return detail_err(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("no connection {:?} in this org", request.connection_id),
            );
        }
        Err(e) => return pg_err(e),
    }
    // The source must be a published config source, the same precondition a
    // delivery has: an extract that lands as an undeclared source would be
    // refused at ingest anyway, an hour later and off the caller's screen.
    match dataset_active_config(&client, &d).await {
        Ok(config) if !config.sources.contains_key(&request.source) => {
            let known: Vec<&str> = config.sources.keys().map(String::as_str).collect();
            return detail_err(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!(
                    "source {:?} is not declared in the published config (declared: {})",
                    request.source,
                    known.join(", ")
                ),
            );
        }
        Ok(_) => {}
        Err(r) => return *r,
    }
    match pg::enqueue(
        &client,
        &org_row.id,
        Some(d.id.as_str()),
        "extract",
        0,
        None,
        Some(&body),
        principal.provenance(),
    )
    .await
    {
        Ok(job) => (
            StatusCode::ACCEPTED,
            Json(json!({ "job": { "id": job.id, "kind": job.kind, "state": job.state } })),
        )
            .into_response(),
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
    ds: &str,
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
    let d = dataset_or_404(client, &detail, ds).await?;
    let action_id = pg::stage_action(client, &detail.id, Some(&d.id), action, actor)
        .await
        .map_err(|e| Box::new(pg_err(e)))?;
    let _ = pg::audit(
        client,
        actor,
        Some(org),
        None,
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
/// (`POST /v1/orgs/{org}/datasets/{ds}/assertions`). With `apply_now`, also enqueue an
/// incremental rebuild that drains + applies it.
async fn add_assertion(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    let action_id = match steward_stage(&client, &principal.actor, &org, &ds, &action).await {
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    if let Err(e) = refuse_unless_active(&org_row, "assertions") {
        return *e;
    }
    match pg::enqueue(
        &client,
        &org_row.id,
        Some(d.id.as_str()),
        "apply_corrections",
        0,
        Some(&idem),
        Some(&params),
        principal.provenance(),
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
    ds: String,
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
        match steward_stage(&client, &principal.actor, &org, &ds, &action).await {
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    if let Err(e) = refuse_unless_active_dataset(&org_row, &d, "steward actions") {
        return *e;
    }
    match pg::enqueue(
        &client,
        &org_row.id,
        Some(&d.id),
        "apply_corrections",
        0,
        Some(&idem),
        Some(&params),
        principal.provenance(),
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

/// Steward+: exclude a record from golden assembly
/// (`POST /v1/orgs/{org}/datasets/{ds}/records/suppress`).
async fn suppress_record_route(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Json(body): Json<SuppressIn>,
) -> impl IntoResponse {
    record_overlay_action(principal, state, org, ds, body, "suppress", false).await
}

/// Steward+: retract a record suppression
/// (`POST /v1/orgs/{org}/datasets/{ds}/records/unsuppress`).
async fn unsuppress_record_route(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Json(body): Json<SuppressIn>,
) -> impl IntoResponse {
    record_overlay_action(principal, state, org, ds, body, "suppress", true).await
}

/// Steward+: pin a record master for its entity
/// (`POST /v1/orgs/{org}/datasets/{ds}/records/protect`).
async fn protect_record_route(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Json(body): Json<SuppressIn>,
) -> impl IntoResponse {
    record_overlay_action(principal, state, org, ds, body, "protect", false).await
}

/// Steward+: retract a record protection
/// (`POST /v1/orgs/{org}/datasets/{ds}/records/unprotect`).
async fn unprotect_record_route(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Json(body): Json<SuppressIn>,
) -> impl IntoResponse {
    record_overlay_action(principal, state, org, ds, body, "protect", true).await
}

/// Steward+: stage an assertion retraction
/// (`DELETE /v1/orgs/{org}/datasets/{ds}/assertions/{assertion_id}`).
async fn retract_assertion(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, assertion_id)): Path<(String, String, String)>,
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
    match steward_stage(&client, &principal.actor, &org, &ds, &action).await {
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

/// Viewer+: the unsatisfiable rule sets (`GET /v1/orgs/{org}/datasets/{ds}/assertions:contradictions`,
/// normalized to `/assertions/contradictions`): every active `never` whose
/// endpoints fall inside one `always`-connected component (S4.4.1
/// `check_contradiction_1`). Returns `{ contradictions: [...] }` — empty when the
/// ledger is satisfiable or absent.
async fn assertions_contradictions(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Viewer) {
        return e.into_response();
    }
    let session = match dataset_lake(&state, &org, &ds).await {
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
/// (`POST /v1/orgs/{org}/datasets/{ds}/golden-records/{entity_id}/unmerge`). `records` is the
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
    Path((org, ds, entity_id)): Path<(String, String, String)>,
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
    let session = match dataset_lake(&state, &org, &ds).await {
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
        match steward_stage(&client, &principal.actor, &org, &ds, &action).await {
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    match pg::enqueue(
        &client,
        &org_row.id,
        Some(d.id.as_str()),
        "apply_corrections",
        0,
        Some(&idem),
        Some(&params),
        principal.provenance(),
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
/// (`GET /v1/orgs/{org}/datasets/{ds}/staged`).
async fn staged_count(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    // Scope-correct before anything is served: an unknown dataset 404s.
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    match pg::pending_count(&client, &d.id).await {
        Ok(n) => Json(json!({ "pending_count": n })).into_response(),
        Err(e) => pg_err(e),
    }
}

#[derive(Deserialize)]
struct ReviewResolveIn {
    resolution: String,
}

/// Steward+: stage a review resolution
/// (`POST /v1/orgs/{org}/datasets/{ds}/reviews/{review_id}/resolve`; the Python
/// `{review_id}:resolve` action). `resolution` is match | no_match | dismiss.
async fn resolve_review(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds, review_id)): Path<(String, String, String)>,
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
    match steward_stage(&client, &principal.actor, &org, &ds, &action).await {
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
/// (`POST /v1/orgs/{org}/datasets/{ds}/reviews/bulk-resolve`). 1..=200 items; with `apply_now`
/// also enqueues the drain job.
async fn bulk_resolve_reviews(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    let mut action_ids = Vec::new();
    for item in &body.items {
        let action = json!({
            "type": "resolve_review",
            "review_id": item.review_id,
            "resolution": item.resolution,
        });
        match pg::stage_action(
            &client,
            &org_row.id,
            Some(d.id.as_str()),
            &action,
            &principal.actor,
        )
        .await
        {
            Ok(id) => action_ids.push(id),
            Err(e) => return pg_err(e),
        }
    }
    let _ = pg::audit(
        &client,
        &principal.actor,
        Some(&org),
        None,
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
            Some(d.id.as_str()),
            "apply_corrections",
            0,
            Some(&idem),
            Some(&params),
            principal.provenance(),
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
struct MergeOverrideIn {
    attribute: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    clear: bool,
}

#[derive(Deserialize)]
struct MergeGroupIn {
    records: Vec<String>,
    master: String,
    #[serde(default)]
    overrides: Vec<MergeOverrideIn>,
    #[serde(default)]
    review_id: Option<String>,
    #[serde(default)]
    apply_now: bool,
}

/// Steward+: merge a GROUP of records from the review dialog
/// (`POST /v1/orgs/{org}/datasets/{ds}/merges`). One bundle, staged together so
/// one drain applies it atomically from the steward's point of view:
///
///   - an `always` assertion star to the chosen master (every non-master
///     record), which is what forces the merge at the next reconcile;
///   - `protect_record` on the master, so the read path elects it;
///   - each field override as `override_field_by_record` keyed by the MASTER's
///     record_key — the merge's entity id is minted at reconcile and cannot be
///     known here, so the pin rides the record and the assemble-time UPDATEs
///     resolve it through the fresh membership;
///   - `resolve_review(match)` when the dialog names its review.
///
/// The bundle's origin is derived, not declared: a `review_id` means the
/// detected-group dialog (`review`), its absence means an arbitrary selection
/// (`manual`, spec 2.10). The origin lands in the assertion notes and the
/// audit detail, so a manual merge is distinguishable from a scored one in
/// both stores — and there is no client-supplied flag to spoof.
///
/// Overrides carry no record_key of their own BY DESIGN: "copy this member's
/// value" pins the VALUE against the master, so it survives the source record
/// later leaving the entity. 2..=50 distinct records, 0..=11 overrides, master
/// must be one of the records.
async fn merge_group(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
    Json(body): Json<MergeGroupIn>,
) -> impl IntoResponse {
    if let Err(e) = guard(&principal, &org, Role::Steward) {
        return e.into_response();
    }
    let records: Vec<&str> = body.records.iter().map(String::as_str).collect();
    let distinct: std::collections::BTreeSet<&&str> = records.iter().collect();
    if records.len() < 2
        || records.len() > 50
        || distinct.len() != records.len()
        || records.iter().any(|r| r.is_empty())
    {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "records must be 2..=50 distinct, non-empty record keys",
        );
    }
    if !records.contains(&body.master.as_str()) {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "master must be one of records",
        );
    }
    let override_attrs: std::collections::BTreeSet<&str> = body
        .overrides
        .iter()
        .map(|o| o.attribute.as_str())
        .collect();
    if override_attrs.len() != body.overrides.len()
        || !override_attrs.iter().all(|a| OVERRIDABLE_ATTRS.contains(a))
    {
        return detail_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "each override needs a distinct attribute in the golden field set",
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
    };
    let manual = body.review_id.is_none();
    let note = if manual {
        "manual merge (no detected group)"
    } else {
        "review merge dialog"
    };
    let mut actions: Vec<Value> = Vec::new();
    for record in &records {
        if *record == body.master {
            continue;
        }
        actions.push(json!({
            "type": "add_assertion",
            "kind": "always",
            "a": body.master,
            "b": record,
            "created_by": principal.actor,
            "note": note,
        }));
    }
    actions.push(json!({
        "type": "protect_record",
        "record_key": body.master,
        "created_by": principal.actor,
    }));
    for o in &body.overrides {
        actions.push(json!({
            "type": "override_field_by_record",
            "record_key": body.master,
            "attribute": o.attribute,
            // `clear: true` and a NULL value mean the same thing to the apply
            // arm; send value only when setting.
            "value": if o.clear { None } else { o.value.clone() },
            "created_by": principal.actor,
        }));
    }
    if let Some(review_id) = &body.review_id {
        actions.push(json!({
            "type": "resolve_review",
            "review_id": review_id,
            "resolution": "match",
        }));
    }
    let mut action_ids = Vec::with_capacity(actions.len());
    for action in &actions {
        match pg::stage_action(
            &client,
            &org_row.id,
            Some(d.id.as_str()),
            action,
            &principal.actor,
        )
        .await
        {
            Ok(id) => action_ids.push(id),
            Err(e) => return pg_err(e),
        }
    }
    let _ = pg::audit(
        &client,
        &principal.actor,
        Some(&org),
        None,
        "steward.merge_group",
        &json!({
            "records": records.len(),
            "master": body.master,
            "overrides": body.overrides.len(),
            "action_ids": action_ids.len(),
            "origin": if manual { "manual" } else { "review" },
        }),
    )
    .await;
    let mut resp = json!({ "status": "staged", "action_ids": action_ids });
    if body.apply_now {
        let idem = format!("merge-group-apply:{}", er_ids::new_v7());
        let params = json!({ "skip_ingest": true });
        match pg::enqueue(
            &client,
            &org_row.id,
            Some(d.id.as_str()),
            "apply_corrections",
            0,
            Some(&idem),
            Some(&params),
            principal.provenance(),
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
/// (`POST /v1/orgs/{org}/datasets/{ds}/mass-update`). 1..=200 items; each sets (`value`) or
/// clears (`clear: true`) one golden field for one entity. With `apply_now` also
/// enqueues the drain (`apply_corrections`), which re-asssembles golden over the
/// staged overrides. Durable + auditable; the matcher/scores are untouched.
async fn mass_update(
    principal: Principal,
    State(state): State<AppState>,
    Path((org, ds)): Path<(String, String)>,
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
    let d = match dataset_or_404(&client, &org_row, &ds).await {
        Ok(row) => row,
        Err(r) => return *r,
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
        match pg::stage_action(
            &client,
            &org_row.id,
            Some(d.id.as_str()),
            &action,
            &principal.actor,
        )
        .await
        {
            Ok(id) => action_ids.push(id),
            Err(e) => return pg_err(e),
        }
    }
    let _ = pg::audit(
        &client,
        &principal.actor,
        Some(&org),
        None,
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
            Some(d.id.as_str()),
            "apply_corrections",
            0,
            Some(&idem),
            Some(&params),
            principal.provenance(),
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
                None,
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
                None,
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
        .route("/readyz", get(readyz))
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
        .route(
            "/v1/orgs/{org}/datasets",
            get(list_datasets_route).post(create_dataset),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}",
            get(get_dataset_route).patch(update_dataset),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/suspend",
            post(suspend_dataset),
        )
        .route("/v1/orgs/{org}/datasets/{ds}/resume", post(resume_dataset))
        .route(
            "/v1/orgs/{org}/datasets/{ds}/jobs",
            post(create_job).get(list_jobs),
        )
        .route("/v1/orgs/{org}/datasets/{ds}/jobs/{job_id}", get(get_job))
        // axum/matchit requires a param to be a whole segment, so the Python
        // `{job_id}:cancel` action becomes a `/cancel` sub-path here.
        .route(
            "/v1/orgs/{org}/datasets/{ds}/jobs/{job_id}/cancel",
            post(cancel_job),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/jobs/{job_id}/resume",
            post(resume_job),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/jobs/{job_id}/tasks",
            get(list_job_tasks),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/jobs/{job_id}/tasks/{task_id}/clear",
            post(clear_job_task),
        )
        // The §6.4 attempt channel (fallback-token flavour): machine-to-machine,
        // authenticated by the per-attempt token rather than a bearer
        // principal; in-namespace only once deployed (the BFF never forwards
        // /v1/internal/*, and tests/mcp_manifest.rs exempts the prefix).
        .route(
            "/v1/internal/attempts/{attempt_id}",
            get(attempt_work_order),
        )
        .route(
            "/v1/internal/attempts/{attempt_id}/start",
            post(attempt_start),
        )
        .route(
            "/v1/internal/attempts/{attempt_id}/heartbeat",
            post(attempt_heartbeat),
        )
        .route(
            "/v1/internal/attempts/{attempt_id}/finish",
            post(attempt_finish),
        )
        .route("/v1/orgs/{org}/api-keys", post(create_key))
        .route(
            "/v1/orgs/{org}/api-keys/{key_id}",
            axum::routing::delete(revoke_key),
        )
        .route("/v1/orgs/{org}/datasets/{ds}/models", get(models_index))
        .route(
            "/v1/orgs/{org}/datasets/{ds}/models/versions",
            post(create_model_version),
        )
        // A sub-path, not `models:compile`: matchit cannot match a param and a
        // literal in one segment. The app()-level rewrite accepts the BFF's
        // `:compile` spelling and lands it here.
        .route(
            "/v1/orgs/{org}/datasets/{ds}/models/compile",
            post(compile_model_version),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/models/{name}/versions/{version}",
            get(get_model_version),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/models/{name}/versions/{version}/evaluation",
            get(model_version_evaluation),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/models/{name}/versions/{version}/activate",
            post(activate_model_version),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/config/versions",
            post(create_config_version).get(list_config_versions),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/config/versions/{version}",
            get(get_config_version),
        )
        // Python's `{version}:publish` action -> a `/publish` sub-path (matchit).
        .route(
            "/v1/orgs/{org}/datasets/{ds}/config/versions/{version}/publish",
            post(publish_config_version),
        )
        .route("/v1/orgs/{org}/datasets/{ds}/config", get(active_config))
        .route(
            "/v1/orgs/{org}/datasets/{ds}/lexicon",
            get(get_lexicon).put(put_lexicon),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/ignore-words",
            get(get_ignore_words).put(put_ignore_words),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/golden-records",
            get(golden_records_list),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/golden-records/{entity_id}",
            get(golden_record_detail),
        )
        // Python's `{entity_id}:unmerge` action -> an `/unmerge` sub-path (matchit).
        .route(
            "/v1/orgs/{org}/datasets/{ds}/golden-records/{entity_id}/unmerge",
            post(unmerge_entity),
        )
        .route("/v1/orgs/{org}/datasets/{ds}/metrics", get(org_metrics))
        .route(
            "/v1/orgs/{org}/datasets/{ds}/data-quality",
            get(org_data_quality),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/duplicates",
            get(duplicates_list),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/match-scores",
            get(match_scores_list),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/match-preview",
            get(match_preview),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/blocking-estimate",
            get(blocking_estimate),
        )
        .route("/v1/orgs/{org}/datasets/{ds}/resolve", post(resolve_record))
        .route(
            "/v1/orgs/{org}/datasets/{ds}/records/ingest",
            post(push_records),
        )
        .route("/v1/orgs/{org}/datasets/{ds}/records", get(records_by_key))
        .route("/v1/orgs/{org}/datasets/{ds}/reviews", get(reviews_list))
        .route(
            "/v1/orgs/{org}/datasets/{ds}/reviews/bulk-resolve",
            post(bulk_resolve_reviews),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/mass-update",
            post(mass_update),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/records/suppress",
            post(suppress_record_route),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/records/unsuppress",
            post(unsuppress_record_route),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/records/protect",
            post(protect_record_route),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/records/unprotect",
            post(unprotect_record_route),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/reviews/{review_id}",
            get(review_detail),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/reviews/{review_id}/merge-group",
            get(review_merge_group),
        )
        .route("/v1/orgs/{org}/datasets/{ds}/merges", post(merge_group))
        .route(
            "/v1/orgs/{org}/datasets/{ds}/reviews/{review_id}/resolve",
            post(resolve_review),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/assertions",
            get(assertions_list).post(add_assertion),
        )
        // Python's `assertions:contradictions` -> a `/contradictions` sub-path.
        .route(
            "/v1/orgs/{org}/datasets/{ds}/assertions/contradictions",
            get(assertions_contradictions),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/assertions/{assertion_id}",
            axum::routing::delete(retract_assertion),
        )
        .route("/v1/orgs/{org}/datasets/{ds}/staged", get(staged_count))
        // The upload needs its own body limit: axum's default (2 MiB) would
        // reject every real delivery, and the limit must be enforced by the
        // server rather than trusted from the BFF.
        .route(
            "/v1/orgs/{org}/datasets/{ds}/imports",
            get(imports_list)
                .post(upload_import)
                .layer(axum::extract::DefaultBodyLimit::max(max_import_bytes)),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/imports/recent",
            get(imports_recent),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/s3-ingest",
            get(s3_ingest_state),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/entity-links",
            post(create_entity_link),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/entity-links/{link_id}",
            axum::routing::delete(delete_entity_link),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/entities/{entity_id}/links",
            get(entity_links),
        )
        .route("/v1/orgs/{org}/datasets/{ds}/runs", get(runs_list))
        .route(
            "/v1/orgs/{org}/datasets/{ds}/runs/{run_id}/entity-events",
            get(run_entity_events),
        )
        .route("/v1/orgs/{org}/tags", get(tags_index).post(create_tag))
        .route(
            "/v1/orgs/{org}/tags/{tag_id}",
            axum::routing::delete(retire_tag),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/tags/{tag_id}/apply",
            post(apply_tag),
        )
        .route("/v1/orgs/{org}/datasets/{ds}/taggings", get(taggings_index))
        .route(
            "/v1/orgs/{org}/datasets/{ds}/tags/{tag_id}/unapply",
            post(unapply_tag),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/tags/{tag_id}/subjects",
            get(tag_subjects),
        )
        .route("/v1/orgs/{org}/datasets/{ds}/merge-plans", get(merge_plans))
        .route(
            "/v1/orgs/{org}/datasets/{ds}/schedules",
            get(schedules_index).post(create_schedule),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/labeling/next",
            get(labeling_next),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/labels",
            post(create_label).get(labels_list),
        )
        .route("/v1/orgs/{org}/datasets/{ds}/insights", get(insights))
        .route(
            "/v1/orgs/{org}/datasets/{ds}/insights/sources",
            get(insight_sources),
        )
        .route("/v1/orgs/{org}/datasets/{ds}/quality", get(quality))
        .route(
            "/v1/orgs/{org}/datasets/{ds}/profile",
            get(dataset_profile).post(dataset_profile_run),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/segments",
            get(segments_catalog),
        )
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
            "/v1/orgs/{org}/connections",
            get(salesforce_connections_index).post(create_salesforce_connection),
        )
        .route(
            "/v1/orgs/{org}/connections/{connection_id}",
            axum::routing::delete(delete_salesforce_connection),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/extract",
            post(dataset_extract_run),
        )
        .route(
            "/v1/orgs/{org}/datasets/{ds}/schedules/{schedule_id}",
            axum::routing::patch(update_schedule).delete(delete_schedule),
        )
        // INSIDE the router, which is the whole point: `MatchedPath` is
        // inserted before a route service runs, so a layer here sees the route
        // TEMPLATE (`/v1/orgs/{org}/jobs`) rather than the populated path. The
        // `:action` normalizer in `app` is pre-routing and cannot. See the
        // `telemetry` module.
        //
        // Order note: in axum the LAST `.layer` is the outermost, so request-id
        // minting wraps the trace layer and an id exists before a span is made.
        .layer(
            tower_http::trace::TraceLayer::new_for_http()
                .make_span_with(telemetry::make_span)
                .on_request(())
                .on_response(telemetry::on_response)
                .on_failure(telemetry::on_failure),
        )
        .layer(tower_http::request_id::PropagateRequestIdLayer::x_request_id())
        .layer(tower_http::request_id::SetRequestIdLayer::x_request_id(
            tower_http::request_id::MakeRequestUuid,
        ))
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
    // Present only when this process runs as a pod; locally the attempt
    // channel stays fallback-token-only.
    let state = state.with_sa_verifier(satoken::Verifier::in_cluster());
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
            dataset_id: "d1".into(),
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
    use super::{
        ACTING_USER_MAX, acceptable_traceparent, compose_actor, forwarded_actor,
        forwarded_traceparent, rewrite_action_path,
    };

    /// Build request parts carrying the given `x-acting-user` value.
    fn parts_with(header: Option<&str>) -> axum::http::request::Parts {
        let mut builder = axum::http::Request::builder().uri("/");
        if let Some(value) = header {
            builder = builder.header(super::ACTING_USER_HEADER, value);
        }
        builder.body(()).unwrap().into_parts().0
    }

    /// Build request parts carrying the given `traceparent` value.
    fn parts_with_traceparent(header: Option<&str>) -> axum::http::request::Parts {
        let mut builder = axum::http::Request::builder().uri("/");
        if let Some(value) = header {
            builder = builder.header(super::TRACEPARENT_HEADER, value);
        }
        builder.body(()).unwrap().into_parts().0
    }

    /// A well-formed context the BFF would send.
    const GOOD_TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn a_well_formed_traceparent_is_stored_verbatim() {
        assert_eq!(
            forwarded_traceparent(&parts_with_traceparent(Some(GOOD_TRACEPARENT))).as_deref(),
            Some(GOOD_TRACEPARENT)
        );
        // Surrounding whitespace is trimmed, not rejected — same rule as the
        // acting-user header, because the same proxies add the same padding.
        assert_eq!(
            forwarded_traceparent(&parts_with_traceparent(Some(
                "  00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01  "
            )))
            .as_deref(),
            Some(GOOD_TRACEPARENT)
        );
        // Absent: a null trace_context, which is exactly today's behaviour.
        assert!(forwarded_traceparent(&parts_with_traceparent(None)).is_none());
        // The sampled flag is not interpreted here — an unsampled context is
        // still a valid parent, and the sampling decision belongs to the
        // collector, not to whether we record the link.
        assert!(
            forwarded_traceparent(&parts_with_traceparent(Some(
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00"
            )))
            .is_some()
        );
    }

    #[test]
    fn a_malformed_traceparent_is_ignored_rather_than_stored() {
        assert!(acceptable_traceparent(GOOD_TRACEPARENT));

        for (bad, why) in [
            ("", "empty"),
            ("not-a-traceparent", "wrong shape"),
            // All-zero trace and span ids are invalid per the W3C spec; storing
            // one would produce a trace nothing can ever be joined to.
            (
                "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
                "zero trace id",
            ),
            (
                "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
                "zero span id",
            ),
            // `ff` is forbidden by the spec.
            (
                "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
                "forbidden version",
            ),
            // A future version may carry fields we would be guessing at.
            (
                "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
                "unknown version",
            ),
            // Uppercase hex would let one context arrive as two distinct strings.
            (
                "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
                "uppercase",
            ),
            (
                "00-4bf92f3577b34da6a3ce929d0e0e473-00f067aa0ba902b7-01",
                "short trace id",
            ),
            (
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra",
                "trailing field",
            ),
            (
                "00-4bf92f3577b34da6a3ce929d0e0e47g6-00f067aa0ba902b7-01",
                "non-hex",
            ),
        ] {
            assert!(!acceptable_traceparent(bad), "{why} should be refused");
            assert!(
                forwarded_traceparent(&parts_with_traceparent(Some(bad))).is_none(),
                "{why} should not reach the column"
            );
        }
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
