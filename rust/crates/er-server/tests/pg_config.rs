//! config_versions CRUD over the real Postgres backend (behind `ER_TEST_PG_DSN`):
//! create a draft (validate + hash + max+1), list (no yaml), get detail (with
//! yaml), the missing-version 404s, active-config 404 before publish, and the
//! admin role boundary — all through the HTTP surface. Publish is covered by
//! `tests/pg_publish.rs`. Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_config

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

/// A full, structurally-valid engine config (the er-config reference fixture).
const DEFAULT_YAML: &str = include_str!("../../er-config/tests/fixtures/default.yaml");

fn req(method: &str, uri: &str, bearer: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .unwrap()
}

fn create_body(yaml: &str) -> Body {
    Body::from(serde_json::json!({ "yaml": yaml }).to_string())
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn config_versions_crud() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres config e2e (set ER_TEST_PG_DSN to run)");
        return;
    };

    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS config_versions; DROP TABLE IF EXISTS api_keys; \
             DROP TABLE IF EXISTS jobs; DROP TABLE IF EXISTS orgs;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::ensure_org(&client, "acme", "/configs/acme.yaml")
        .await
        .unwrap();

    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    // create draft v1: 201 with the assigned version + hash + draft state.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/config/versions",
            "op-secret",
            create_body(DEFAULT_YAML),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v1 = json_body(resp).await;
    assert_eq!(v1["version"], 1);
    assert_eq!(v1["state"], "draft");
    let hash = v1["config_hash"].as_str().unwrap().to_string();
    assert_eq!(hash.len(), 64, "config_hash is a sha-256 hex digest");

    // create draft v2: version increments per org.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/config/versions",
            "op-secret",
            create_body(DEFAULT_YAML),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(json_body(resp).await["version"], 2);

    // invalid config (unknown block): 422.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/config/versions",
            "op-secret",
            create_body("tenant: x\nnot_a_block: 1\n"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // list: newest first, no yaml body.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/config/versions",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let list = json_body(resp).await;
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["version"], 2);
    assert_eq!(arr[1]["version"], 1);
    assert_eq!(arr[0]["state"], "draft");
    assert!(arr[0]["tier"].is_null());
    assert!(arr[0]["published_at"].is_null());
    assert!(arr[0]["yaml"].is_null(), "list omits the yaml body");

    // detail: includes yaml, same hash as create.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/config/versions/1",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let detail = json_body(resp).await;
    assert_eq!(detail["version"], 1);
    assert_eq!(detail["config_hash"], hash);
    assert!(detail["yaml"].as_str().unwrap().contains("tenant:"));

    // detail of a non-existent / non-numeric version: 404.
    for bad in ["99", "abc"] {
        let resp = app
            .clone()
            .oneshot(req(
                "GET",
                &format!("/v1/orgs/acme/config/versions/{bad}"),
                "op-secret",
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "version {bad}");
    }

    // active config: 404 while nothing is published.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/config",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Role boundary: create needs admin; a steward key is 403, a viewer lists.
    let steward_key = mint_key(&app, "steward").await;
    let viewer_key = mint_key(&app, "viewer").await;

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/config/versions",
            &steward_key,
            create_body(DEFAULT_YAML),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = app
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/config/versions",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

async fn mint_key(app: &axum::Router, role: &str) -> String {
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/api-keys",
            "op-secret",
            Body::from(serde_json::json!({ "role": role }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    json_body(resp).await["key"].as_str().unwrap().to_string()
}
