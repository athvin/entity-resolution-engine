//! Schedules CRUD over the real Postgres backend (behind `ER_TEST_PG_DSN`):
//! create (kind + cron validation), list, enable/disable (config-owned refuse),
//! delete, and the admin role boundary — through the HTTP surface. Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_schedules

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
async fn schedules_crud_and_role_boundary() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres schedules e2e (set ER_TEST_PG_DSN to run)");
        return;
    };
    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS schedules, config_versions, api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::ensure_org(&client, "acme", "/configs/acme.yaml")
        .await
        .unwrap();
    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    // create: valid kind + cron -> 201.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/schedules",
            "op-secret",
            Body::from(r#"{"kind":"run_all_full","cron":"0 3 * * *"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = json_body(resp).await;
    let schedule_id = created["schedule_id"].as_str().unwrap().to_string();
    assert_eq!(created["kind"], "run_all_full");

    // bad kind / bad cron -> 422.
    for body in [
        r#"{"kind":"nope","cron":"0 3 * * *"}"#,
        r#"{"kind":"run_all_full","cron":"not a cron"}"#,
    ] {
        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                "/v1/orgs/acme/schedules",
                "op-secret",
                Body::from(body),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }

    // list: the one schedule, api-sourced, enabled.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/schedules",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let list = json_body(resp).await;
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["source"], "api");
    assert_eq!(arr[0]["enabled"], true);

    // disable it.
    let resp = app
        .clone()
        .oneshot(req(
            "PATCH",
            &format!("/v1/orgs/acme/schedules/{schedule_id}"),
            "op-secret",
            Body::from(r#"{"enabled":false}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["enabled"], false);

    // patch an unknown schedule -> 409.
    let resp = app
        .clone()
        .oneshot(req(
            "PATCH",
            "/v1/orgs/acme/schedules/does-not-exist",
            "op-secret",
            Body::from(r#"{"enabled":true}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // delete (idempotent: second delete reports deleted=false).
    let resp = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/v1/orgs/acme/schedules/{schedule_id}"),
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["deleted"], true);
    let resp = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/v1/orgs/acme/schedules/{schedule_id}"),
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["deleted"], false);

    // Role boundary: a viewer key lists but cannot create (admin).
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
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/schedules",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/schedules",
            &viewer_key,
            Body::from(r#"{"kind":"run_all_full","cron":"0 3 * * *"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
