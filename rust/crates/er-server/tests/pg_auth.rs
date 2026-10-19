//! API-key auth over the real Postgres backend (behind `ER_TEST_PG_DSN`): mint a
//! key through the HTTP surface, authenticate requests with it, and prove the
//! role lattice + org scoping + revocation (`auth.py` `authenticate` +
//! `Principal.require`). Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_auth

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

fn req(method: &str, uri: &str, bearer: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .unwrap()
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn api_key_auth_role_scope_and_revocation() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres auth e2e (set ER_TEST_PG_DSN to run)");
        return;
    };

    // Clean slate (api_keys/jobs reference orgs) + schema + the FK org.
    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS staged_steward_actions, audit_log, schedules, config_versions, api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::ensure_org(&client, "acme", "/configs/acme.yaml")
        .await
        .unwrap();

    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    // Operator mints an admin key and a viewer key for acme.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/api-keys",
            "op-secret",
            Body::from(r#"{"role":"admin"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let minted = json_body(resp).await;
    let admin_key = minted["key"].as_str().unwrap().to_string();
    let admin_key_id = minted["id"].as_str().unwrap().to_string();
    assert_eq!(minted["role"], "admin");
    assert!(admin_key.starts_with(&format!("erk_{admin_key_id}_")));

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/api-keys",
            "op-secret",
            Body::from(r#"{"role":"viewer"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let viewer_key = json_body(resp).await["key"].as_str().unwrap().to_string();

    // operator is not a mintable key role → 422.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/api-keys",
            "op-secret",
            Body::from(r#"{"role":"operator"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // admin key: enqueue (steward+) succeeds.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/jobs",
            &admin_key,
            Body::from(r#"{"kind":"run"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    // viewer key: enqueue is 403 (role below steward)...
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/jobs",
            &viewer_key,
            Body::from(r#"{"kind":"run"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // ...but a viewer can list (viewer+).
    let resp = app
        .clone()
        .oneshot(req("GET", "/v1/orgs/acme/jobs", &viewer_key, Body::empty()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // cross-org scope is a 404 (never enumerates orgs), resolved before any DB.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/other/jobs",
            &admin_key,
            Body::from(r#"{"kind":"run"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // a well-formed but unknown key is a generic 401.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/jobs",
            "erk_01bogus_deadbeef",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // operator revokes the admin key (idempotent).
    let resp = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/v1/orgs/acme/api-keys/{admin_key_id}"),
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["revoked"], true);

    // the revoked admin key no longer authenticates → 401.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/jobs",
            &admin_key,
            Body::from(r#"{"kind":"run"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // a second revoke is a no-op.
    let resp = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/v1/orgs/acme/api-keys/{admin_key_id}"),
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["revoked"], false);

    // missing bearer anywhere is 401.
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/orgs/acme/jobs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
