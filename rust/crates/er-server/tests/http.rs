//! Router-level gates (no network): drive the axum app with oneshot requests.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app};
use tower::ServiceExt;

fn test_app() -> axum::Router {
    app(AppState::new("op-secret".to_string()))
}

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
async fn healthz_is_anonymous_and_ok() {
    let req = Request::builder()
        .uri("/healthz")
        .body(Body::empty())
        .unwrap();
    let resp = test_app().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["status"], "ok");
}

#[tokio::test]
async fn v1_requires_bearer() {
    let no_auth = Request::builder()
        .uri("/v1/orgs")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        test_app().oneshot(no_auth).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    // A credential that is neither the operator token nor a valid `erk_` key is
    // an auth failure (401), not an authorization failure (403) — matching the
    // Python contract (403 is reserved for a valid principal lacking the role).
    let wrong = Request::builder()
        .uri("/v1/orgs")
        .header(header::AUTHORIZATION, "Bearer nope")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        test_app().oneshot(wrong).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn job_lifecycle_enqueue_list_get_cancel() {
    let app = test_app();

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

    // list
    let resp = app
        .clone()
        .oneshot(op_request("GET", "/v1/orgs/acme/jobs", Body::empty()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let jobs = json_body(resp).await;
    assert_eq!(jobs["jobs"].as_array().unwrap().len(), 1);

    // another org sees none
    let resp = app
        .clone()
        .oneshot(op_request("GET", "/v1/orgs/other/jobs", Body::empty()))
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["jobs"].as_array().unwrap().len(), 0);

    // get
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

    // missing job
    let resp = app
        .oneshot(op_request(
            "GET",
            "/v1/orgs/acme/jobs/job-999",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
