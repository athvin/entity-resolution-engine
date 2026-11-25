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

use crate::common;

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
    let Some(db) = common::fresh_db("auth").await else {
        return;
    };
    let dsn = db.dsn().to_string();

    // Clean slate (api_keys/jobs reference orgs) + schema + the FK org.
    let mut client = pg::connect(&dsn).await.unwrap();
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

    // --- Self-service (#866): an org admin manages its own org's keys. ---
    pg::ensure_org(&client, "other", "/configs/other.yaml")
        .await
        .unwrap();
    let mint = |bearer: String, org: &'static str, role: &'static str| {
        let app = app.clone();
        async move {
            app.oneshot(req(
                "POST",
                &format!("/v1/orgs/{org}/api-keys"),
                &bearer,
                Body::from(format!(r#"{{"role":"{role}"}}"#)),
            ))
            .await
            .unwrap()
        }
    };
    // An admin mints for its own org: 201 with the plaintext.
    let resp = mint(admin_key.clone(), "acme", "steward").await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let steward = json_body(resp).await;
    let steward_key = steward["key"].as_str().unwrap().to_string();
    let steward_key_id = steward["id"].as_str().unwrap().to_string();
    assert_eq!(steward["role"], "steward");
    // Another (existing) org is the cross-org 404, never a 403.
    let resp = mint(admin_key.clone(), "other", "viewer").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    // Below admin cannot mint at all.
    for (key, who) in [(&viewer_key, "viewer"), (&steward_key, "steward")] {
        let resp = mint(key.clone(), "acme", "viewer").await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{who} minted a key");
    }
    // The ceiling's hard end: no principal mints an operator key.
    let resp = mint(admin_key.clone(), "acme", "operator").await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    // The minted key works on the org it belongs to (the getting-started call).
    let resp = app
        .clone()
        .oneshot(req("GET", "/v1/orgs/acme", &steward_key, Body::empty()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The list: admin only, metadata only.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/api-keys",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/other/api-keys",
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/api-keys",
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = json_body(resp).await;
    let keys = listed["keys"].as_array().unwrap();
    assert_eq!(keys.len(), 3, "admin, viewer and steward: {listed}");
    let roles: Vec<&str> = keys.iter().map(|k| k["role"].as_str().unwrap()).collect();
    assert_eq!(roles.iter().filter(|r| **r == "steward").count(), 1);
    let text = listed.to_string();
    // The plaintext was shown exactly once, at mint, and the hash never.
    for secret in [&admin_key, &viewer_key, &steward_key] {
        assert!(
            !text.contains(secret.as_str()),
            "a plaintext key was listed"
        );
    }
    assert!(
        keys.iter()
            .all(|k| k.get("key_hash").is_none() && k.get("key").is_none())
    );

    // An admin revokes within its org.
    let resp = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/v1/orgs/acme/api-keys/{steward_key_id}"),
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["revoked"], true);
    let resp = app
        .clone()
        .oneshot(req("GET", "/v1/orgs/acme", &steward_key, Body::empty()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Both halves are audited: the admin's issue and its revoke.
    let audit = pg::audit_rows(
        &client,
        Some("acme"),
        &pg::AuditFilters::default(),
        None,
        100,
    )
    .await
    .unwrap();
    let names = |action: &str| {
        audit
            .iter()
            .filter(|e| e.action == action)
            .filter(|e| e.detail["key_id"] == steward_key_id.as_str())
            .count()
    };
    assert_eq!(names("key.issue"), 1, "{audit:?}");
    assert_eq!(names("key.revoke"), 1, "{audit:?}");

    // admin key: enqueue (steward+) succeeds.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/datasets/default/jobs",
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
            "/v1/orgs/acme/datasets/default/jobs",
            &viewer_key,
            Body::from(r#"{"kind":"run"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // ...but a viewer can list (viewer+).
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/datasets/default/jobs",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // cross-org scope is a 404 (never enumerates orgs), resolved before any DB.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/other/datasets/default/jobs",
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
            "/v1/orgs/acme/datasets/default/jobs",
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
            "/v1/orgs/acme/datasets/default/jobs",
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
                .uri("/v1/orgs/acme/datasets/default/jobs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
