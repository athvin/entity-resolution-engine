//! Steward write staging over the real Postgres backend (behind `ER_TEST_PG_DSN`):
//! assertion create/retract stage actions (the writer-lock-held path — the
//! immediate apply needs the postgres-catalog advisory lock, which is
//! infra-bound), `apply_now` enqueues the drain job, `/staged` counts pending,
//! and the active-org + role guards. Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_steward

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

async fn pending(app: &axum::Router) -> i64 {
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/staged",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    json_body(resp).await["pending_count"].as_i64().unwrap()
}

#[tokio::test]
async fn steward_actions_stage_and_guard() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping steward e2e (set ER_TEST_PG_DSN to run)");
        return;
    };
    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS staged_steward_actions, audit_log, schedules, config_versions, \
             api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::ensure_org(&client, "acme", "/configs/acme.yaml")
        .await
        .unwrap();
    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    assert_eq!(pending(&app).await, 0, "nothing staged yet");

    // stage an always assertion -> 201 staged.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/assertions",
            "op-secret",
            Body::from(r#"{"kind":"always","a":"crm:C001","b":"webforms:W006"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let staged = json_body(resp).await;
    assert_eq!(staged["status"], "staged");
    assert!(staged["action_id"].as_str().is_some());
    assert_eq!(pending(&app).await, 1);

    // bad kind -> 422.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/assertions",
            "op-secret",
            Body::from(r#"{"kind":"maybe","a":"x","b":"y"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // apply_now also enqueues the incremental drain job.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/assertions",
            "op-secret",
            Body::from(r#"{"kind":"never","a":"crm:C003","b":"billing:B003","apply_now":true}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert!(json_body(resp).await["apply_job"].as_str().is_some());
    assert_eq!(pending(&app).await, 2);

    // retract stages a retract action.
    let resp = app
        .clone()
        .oneshot(req(
            "DELETE",
            "/v1/orgs/acme/assertions/some-assertion-id",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["status"], "staged");
    assert_eq!(pending(&app).await, 3);

    // the staged actions were audited.
    let resp = app
        .clone()
        .oneshot(req("GET", "/v1/audit", "op-secret", Body::empty()))
        .await
        .unwrap();
    let actions: Vec<String> = json_body(resp)
        .await
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["action"].as_str().map(str::to_string))
        .collect();
    assert!(actions.iter().any(|a| a == "steward.add_assertion"));
    assert!(actions.iter().any(|a| a == "steward.retract_assertion"));

    // resolve a review (stages a resolve_review action).
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/reviews/gray_band:crm:C001:webforms:W006/resolve",
            "op-secret",
            Body::from(r#"{"resolution":"match"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["status"], "staged");
    assert_eq!(pending(&app).await, 4);

    // bad resolution -> 422.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/reviews/some-id/resolve",
            "op-secret",
            Body::from(r#"{"resolution":"perhaps"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // bulk-resolve stages one action per item.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/reviews/bulk-resolve",
            "op-secret",
            Body::from(
                r#"{"items":[{"review_id":"r1","resolution":"dismiss"},{"review_id":"r2","resolution":"no_match"}]}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        json_body(resp).await["action_ids"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(pending(&app).await, 6);

    // empty bulk -> 422.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/reviews/bulk-resolve",
            "op-secret",
            Body::from(r#"{"items":[]}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // a viewer key cannot stage (steward required).
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
            "POST",
            "/v1/orgs/acme/assertions",
            &viewer_key,
            Body::from(r#"{"kind":"always","a":"x","b":"y"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // steward actions are refused on a non-active org (409).
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
    let resp = app
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/assertions",
            "op-secret",
            Body::from(r#"{"kind":"always","a":"x","b":"y"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}
