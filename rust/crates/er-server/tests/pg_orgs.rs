//! Orgs CRUD over the real Postgres backend (behind `ER_TEST_PG_DSN`): create
//! (BYO + auto-mode 503 + name validation), list, get (incl. the `resources`
//! projection), and the suspend/resume state machine with its 404/409 edges —
//! all through the HTTP surface. Also checks the role boundary on these routes.
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_orgs

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
async fn orgs_crud_and_state_machine() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres orgs e2e (set ER_TEST_PG_DSN to run)");
        return;
    };

    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute("DROP TABLE IF EXISTS config_versions, api_keys, jobs, orgs CASCADE;")
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    // create (BYO): returns {name}, org starts active.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs",
            "op-secret",
            Body::from(
                r#"{"name":"acme","config_path":"/configs/acme.yaml","env":{"ER_DUCKDB_THREADS":"4"}}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(json_body(resp).await["name"], "acme");

    // create (auto, no config_path): the faithful 503.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs",
            "op-secret",
            Body::from(r#"{"name":"auto1"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

    // create with an invalid name: 422.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs",
            "op-secret",
            Body::from(r#"{"name":"Bad_NAME","config_path":"/x.yaml"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // list (operator): sees acme, active, no published config yet.
    let resp = app
        .clone()
        .oneshot(req("GET", "/v1/orgs", "op-secret", Body::empty()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let orgs = json_body(resp).await;
    let arr = orgs.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "acme");
    assert_eq!(arr[0]["state"], "active");
    assert!(arr[0]["active_config_version"].is_null());
    assert!(arr[0]["created_at"].is_string());

    // get: resources projected out of env.
    let resp = app
        .clone()
        .oneshot(req("GET", "/v1/orgs/acme", "op-secret", Body::empty()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let detail = json_body(resp).await;
    assert_eq!(detail["state"], "active");
    assert_eq!(detail["resources"]["duckdb_threads"], 4);
    assert!(detail["resources"]["duckdb_memory_limit"].is_null());
    assert!(detail["drop_root"].is_null());

    // get a missing org: 404.
    let resp = app
        .clone()
        .oneshot(req("GET", "/v1/orgs/ghost", "op-secret", Body::empty()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // suspend (active -> suspended).
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/suspend",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["state"], "suspended");

    // suspend again: 409 (not active).
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/suspend",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // resume (suspended -> active).
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/resume",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["state"], "active");

    // resume again: 409 (not suspended).
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/resume",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // suspend a missing org: 404 (precheck before the state UPDATE).
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/ghost/suspend",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Role boundary: a viewer key can read its org but not the operator surfaces.
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
    let viewer_key = json_body(resp).await["key"].as_str().unwrap().to_string();

    // viewer GET its org detail: 200.
    let resp = app
        .clone()
        .oneshot(req("GET", "/v1/orgs/acme", &viewer_key, Body::empty()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // viewer GET the operator-only org list: 403.
    let resp = app
        .clone()
        .oneshot(req("GET", "/v1/orgs", &viewer_key, Body::empty()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Operator PATCH resources: merges the DuckDB knobs into env.
    let resp = app
        .clone()
        .oneshot(req(
            "PATCH",
            "/v1/orgs/acme/resources",
            "op-secret",
            Body::from(r#"{"duckdb_threads":8,"duckdb_memory_limit":"4GB"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let res = json_body(resp).await;
    assert_eq!(res["duckdb_threads"], 8);
    assert_eq!(res["duckdb_memory_limit"], "4GB");

    // PATCH resources with neither field: 422.
    let resp = app
        .clone()
        .oneshot(req(
            "PATCH",
            "/v1/orgs/acme/resources",
            "op-secret",
            Body::from("{}"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Operator cross-org job console: enqueue one, then see it in /v1/jobs.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/jobs",
            "op-secret",
            Body::from(r#"{"kind":"run"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let resp = app
        .clone()
        .oneshot(req("GET", "/v1/jobs", "op-secret", Body::empty()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(!json_body(resp).await.as_array().unwrap().is_empty());
    // Filter by state, and reject an unknown state.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/jobs?state=queued",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/jobs?state=bogus",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // /v1/jobs is operator-only: a viewer key is 403.
    let resp = app
        .clone()
        .oneshot(req("GET", "/v1/jobs", &viewer_key, Body::empty()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // viewer attempting suspend (requires operator): 403, in-scope.
    let resp = app
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/suspend",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
