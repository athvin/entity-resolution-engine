//! Regression: the `source_priority` master-election policy could never load
//! its source ranks (behind `ER_TEST_PG_DSN`).
//!
//! `GET /v1/orgs/{org}/merge-plans?policy=source_priority` is the one policy
//! that needs the org's config, which it loaded with
//! `pg::get_config_version(&client, org, active)` — passing the org **name**
//! where that query keys on the org **id**. The lookup therefore missed for
//! every org that had a published config, and the handler answered
//! `404 no published config`. Nothing caught it because the other four policies
//! never load the config and this one had no test.
//!
//! The assertion is about where the request gets to: with ranks loading, a
//! source_priority request must reach the lake step and fail there exactly like
//! the default policy does on an org with no lake attached — not short-circuit
//! on its config.
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_source_ranks

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

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

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn source_priority_loads_its_ranks_from_the_active_config() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres source-ranks regression (set ER_TEST_PG_DSN to run)");
        return;
    };

    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS staged_steward_actions, audit_log, schedules, config_versions, api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    let tmp = std::env::temp_dir().join(format!("er_ranks_{}.yaml", std::process::id()));
    let config_path = tmp.to_string_lossy().to_string();
    let _ = std::fs::remove_file(&config_path);

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs",
            "op-secret",
            Body::from(
                serde_json::json!({ "name": "acme", "config_path": config_path }).to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

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
    let admin_key = json_body(resp).await["key"].as_str().unwrap().to_string();

    // Publish a config: the fixture declares crm / billing / webforms with ranks.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/config/versions",
            &admin_key,
            Body::from(serde_json::json!({ "yaml": DEFAULT_YAML }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let version = json_body(resp).await["version"].as_i64().unwrap();
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/v1/orgs/acme/config/versions/{version}/publish"),
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The control: a policy that needs no config gets as far as the lake.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/merge-plans?policy=most_attributes",
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    let control = resp.status();
    let control_detail = json_body(resp).await["detail"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The fix: source_priority must get exactly as far, not stop at its config.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/merge-plans?policy=source_priority",
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    let status = resp.status();
    let detail = json_body(resp).await["detail"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    assert!(
        !detail.contains("no published config"),
        "source_priority could not read the org's published config: {detail}"
    );
    assert_eq!(
        status, control,
        "source_priority stopped somewhere the default policy did not \
         (source_priority: {status} {detail:?}; most_attributes: {control} {control_detail:?})"
    );

    let _ = std::fs::remove_file(&config_path);
}
