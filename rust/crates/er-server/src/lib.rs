//! `er-server` — the Rust control plane (P3), an axum port of the Python
//! `erserver` FastAPI app (`server/src/erserver/api.py`). It replaces FastAPI as
//! the orgs / jobs / config / read-path HTTP surface.
//!
//! SCAFFOLD STATUS: this lands the HTTP skeleton — the router, the `/healthz`
//! anonymous surface (infrastructure.md §4.1), the bearer-token auth boundary,
//! and representative `/v1` routes — so the axum foundation is in place and
//! testable without network. The handlers that need the Postgres-backed job
//! queue (SKIP-LOCKED) and the OIDC BFF are stubbed with `501 Not Implemented`
//! and marked TODO; they land once that infrastructure is available (they can't
//! run in this sandbox). Auth is a SHA-256 bearer check (never argon2), matching
//! the Python contract.

use axum::Json;
use axum::Router;
use axum::extract::{FromRequestParts, Path};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use serde_json::{Value, json};

/// Process-wide server state. The Postgres pool + OIDC verifier will join here
/// when their infrastructure lands; for now only the operator bearer is wired.
#[derive(Clone)]
pub struct AppState {
    /// The operator bearer token (the cross-org admin principal). Empty disables
    /// operator auth (every operator route then 401s).
    pub operator_token: String,
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

/// Operator: list orgs. TODO: read from the Postgres control DB (queue.list_orgs).
async fn list_orgs(_: Operator) -> Json<Value> {
    Json(json!({ "orgs": [] }))
}

/// Operator: enqueue a job for an org. TODO: insert into the SKIP-LOCKED job
/// queue and return the minted job id.
async fn create_job(_: Operator, Path(org): Path<String>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "error": "job queue not yet wired (needs Postgres)",
            "org": org,
        })),
    )
}

/// Operator: list an org's jobs. TODO: read from the Postgres job queue.
async fn list_jobs(_: Operator, Path(org): Path<String>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({ "error": "job queue not yet wired (needs Postgres)", "org": org })),
    )
}

/// Build the control-plane router. `/healthz` is anonymous; every `/v1` route is
/// behind the bearer boundary.
pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/orgs", get(list_orgs))
        .route("/v1/orgs/{org}/jobs", post(create_job).get(list_jobs))
        .with_state(state)
}

/// Read the operator bearer from the environment (`ER_OPERATOR_TOKEN`).
pub fn state_from_env() -> AppState {
    AppState {
        operator_token: std::env::var("ER_OPERATOR_TOKEN").unwrap_or_default(),
    }
}
