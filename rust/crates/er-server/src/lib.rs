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
//! `tests/pg_http.rs` (`ER_TEST_PG_DSN`). OIDC BFF auth + the
//! org/config/webhook/import endpoints are stubbed (501, TODO) — they need
//! OIDC infrastructure that can't run in this sandbox. Auth is a SHA-256 bearer
//! check (never argon2), matching the Python contract.

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

/// An authenticated operator principal — the cross-org admin role. Extracting it
/// enforces `Authorization: Bearer <operator_token>`; a missing header is 401, a
/// wrong token 403 (matching erserver's `operator_only`).
pub struct Operator;

impl FromRequestParts<AppState> for Operator {
    type Rejection = (StatusCode, String);

    // The bearer check is synchronous; the Postgres API-key lookup that lands
    // here later makes it genuinely async. Keep the `async fn` form until then.
    #[allow(clippy::unused_async_trait_impl)]
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok());
        match header {
            None => Err((
                StatusCode::UNAUTHORIZED,
                "missing Authorization".to_string(),
            )),
            Some(h) => {
                let token = h.strip_prefix("Bearer ").unwrap_or(h);
                if !state.operator_token.is_empty() && token == state.operator_token {
                    Ok(Operator)
                } else {
                    Err((StatusCode::FORBIDDEN, "requires operator role".to_string()))
                }
            }
        }
    }
}

/// The anonymous liveness surface (§4.1): the only route that never authenticates.
async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

/// Operator: list orgs. TODO: read from the Postgres control DB (needs infra).
async fn list_orgs(_: Operator) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({ "error": "org registry not yet wired (needs Postgres)" })),
    )
}

#[derive(Deserialize)]
struct EnqueueJob {
    kind: String,
    #[serde(default)]
    priority: i64,
}

/// Operator: enqueue a job for an org (S-queue enqueue).
async fn create_job(
    _: Operator,
    State(state): State<AppState>,
    Path(org): Path<String>,
    Json(body): Json<EnqueueJob>,
) -> impl IntoResponse {
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

/// Operator: list an org's jobs, newest first.
async fn list_jobs(
    _: Operator,
    State(state): State<AppState>,
    Path(org): Path<String>,
) -> axum::response::Response {
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

/// Operator: one job by id (404 if absent / not this org's).
async fn get_job(
    _: Operator,
    State(state): State<AppState>,
    Path((org, job_id)): Path<(String, String)>,
) -> impl IntoResponse {
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

/// Operator: cancel a job (idempotent-ish: queued/running -> canceled).
async fn cancel_job(
    _: Operator,
    State(state): State<AppState>,
    Path((org, job_id)): Path<(String, String)>,
) -> impl IntoResponse {
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

/// Build the control-plane router. `/healthz` is anonymous; every `/v1` route is
/// behind the bearer boundary.
pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/orgs", get(list_orgs))
        .route("/v1/orgs/{org}/jobs", post(create_job).get(list_jobs))
        .route("/v1/orgs/{org}/jobs/{job_id}", get(get_job))
        // axum/matchit requires a param to be a whole segment, so the Python
        // `{job_id}:cancel` action becomes a `/cancel` sub-path here.
        .route("/v1/orgs/{org}/jobs/{job_id}/cancel", post(cancel_job))
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
