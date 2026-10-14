//! Webhooks CRUD over the real Postgres backend (behind `ER_TEST_PG_DSN`):
//! register (secret returned once), list (secret withheld), delete (idempotent),
//! plus the url/org validations. Delivery itself is infra-bound (a background
//! signer/poster), so this gates the registration surface only. Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_webhooks

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

fn op(method: &str, uri: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer op-secret")
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
async fn webhooks_crud() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping webhooks e2e (set ER_TEST_PG_DSN to run)");
        return;
    };
    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS staged_steward_actions, audit_log, webhooks, schedules, \
             config_versions, api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::ensure_org(&client, "acme", "/configs/acme.yaml")
        .await
        .unwrap();
    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    // Empty to start.
    let resp = app
        .clone()
        .oneshot(op("GET", "/v1/orgs/acme/webhooks", Body::empty()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await.as_array().unwrap().len(), 0);

    // Create: the secret is generated + returned once.
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/webhooks",
            Body::from(r#"{"url":"https://hooks.example/acme","events":["run.succeeded","review.opened"]}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = json_body(resp).await;
    let webhook_id = created["webhook_id"].as_str().unwrap().to_string();
    assert!(created["secret"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(
        created["events"],
        serde_json::json!(["run.succeeded", "review.opened"])
    );
    assert_eq!(created["active"], true);

    // A caller-supplied secret is honored.
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/webhooks",
            Body::from(
                r#"{"url":"https://hooks.example/two","events":[],"secret":"my-own-secret"}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(json_body(resp).await["secret"], "my-own-secret");

    // List shows both, with NO secret field.
    let resp = app
        .clone()
        .oneshot(op("GET", "/v1/orgs/acme/webhooks", Body::empty()))
        .await
        .unwrap();
    let rows = json_body(resp).await;
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter().all(|w| w.get("secret").is_none()),
        "secret is never listed"
    );
    assert!(
        rows.iter()
            .any(|w| w["url"] == "https://hooks.example/acme")
    );

    // Delete is idempotent.
    let resp = app
        .clone()
        .oneshot(op(
            "DELETE",
            &format!("/v1/orgs/acme/webhooks/{webhook_id}"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["deleted"], true);
    let resp = app
        .clone()
        .oneshot(op(
            "DELETE",
            &format!("/v1/orgs/acme/webhooks/{webhook_id}"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["deleted"], false);

    // Validation: empty url -> 422.
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/webhooks",
            Body::from(r#"{"url":"","events":[]}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Unknown org -> 404.
    let resp = app
        .oneshot(op(
            "POST",
            "/v1/orgs/ghost/webhooks",
            Body::from(r#"{"url":"https://x","events":[]}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
