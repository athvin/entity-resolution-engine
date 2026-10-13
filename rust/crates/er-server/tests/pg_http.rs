//! The HTTP surface over the real Postgres queue (behind `ER_TEST_PG_DSN`):
//! drives the axum app built with `AppState::with_pg`, so every request connects
//! to Postgres per-request exactly as production does. This is the queue of
//! `pg_e2e.rs` seen through `/v1/orgs/{org}/jobs`. Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_http

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

fn op_request(method: &str, uri: &str, body: Body) -> Request<Body> {
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
async fn job_lifecycle_over_postgres_http() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres HTTP e2e (set ER_TEST_PG_DSN to run)");
        return;
    };

    // Clean slate + schema + the org the FK requires.
    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute("DROP TABLE IF EXISTS config_versions, api_keys, jobs, orgs CASCADE;")
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::ensure_org(&client, "acme", "/configs/acme.yaml")
        .await
        .unwrap();

    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    // Missing bearer is still 401 on the Postgres backend (auth precedes the DB).
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/orgs/acme/jobs")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"kind":"run"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // enqueue
    let resp = app
        .clone()
        .oneshot(op_request(
            "POST",
            "/v1/orgs/acme/jobs",
            Body::from(r#"{"kind":"run","priority":5}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let job = json_body(resp).await;
    let job_id = job["job_id"].as_str().unwrap().to_string();
    assert_eq!(job["state"], "queued");
    assert_eq!(job["kind"], "run");
    assert_eq!(job["org"], "acme");
    assert_eq!(job["priority"], 5);

    // list sees exactly the one job
    let resp = app
        .clone()
        .oneshot(op_request("GET", "/v1/orgs/acme/jobs", Body::empty()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let jobs = json_body(resp).await;
    assert_eq!(jobs["jobs"].as_array().unwrap().len(), 1);
    assert_eq!(jobs["jobs"][0]["job_id"], job_id);

    // an org with no jobs lists empty
    let resp = app
        .clone()
        .oneshot(op_request("GET", "/v1/orgs/other/jobs", Body::empty()))
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["jobs"].as_array().unwrap().len(), 0);

    // get the job by id
    let resp = app
        .clone()
        .oneshot(op_request(
            "GET",
            &format!("/v1/orgs/acme/jobs/{job_id}"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["job_id"], job_id);

    // a job id scoped to the wrong org is a 404
    let resp = app
        .clone()
        .oneshot(op_request(
            "GET",
            &format!("/v1/orgs/other/jobs/{job_id}"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // cancel
    let resp = app
        .clone()
        .oneshot(op_request(
            "POST",
            &format!("/v1/orgs/acme/jobs/{job_id}/cancel"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["state"], "canceled");

    // the cancel persisted
    let resp = app
        .clone()
        .oneshot(op_request(
            "GET",
            &format!("/v1/orgs/acme/jobs/{job_id}"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["state"], "canceled");

    // resume the canceled job -> back to queued
    let resp = app
        .clone()
        .oneshot(op_request(
            "POST",
            &format!("/v1/orgs/acme/jobs/{job_id}/resume"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["state"], "queued");
    let resp = app
        .clone()
        .oneshot(op_request(
            "GET",
            &format!("/v1/orgs/acme/jobs/{job_id}"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["state"], "queued");

    // resuming a queued (non-failed/canceled) job is a 409
    let resp = app
        .clone()
        .oneshot(op_request(
            "POST",
            &format!("/v1/orgs/acme/jobs/{job_id}/resume"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // a genuinely absent job is a 404
    let resp = app
        .oneshot(op_request(
            "GET",
            "/v1/orgs/acme/jobs/01BOGUSULIDDOESNOTEXIST00",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
