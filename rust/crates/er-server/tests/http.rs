//! Router-level gates (no network): drive the axum app with oneshot requests.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app};
use tower::ServiceExt;

fn test_app() -> axum::Router {
    app(AppState {
        operator_token: "op-secret".to_string(),
    })
}

async fn status_of(req: Request<Body>) -> StatusCode {
    test_app().oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn healthz_is_anonymous_and_ok() {
    let req = Request::builder()
        .uri("/healthz")
        .body(Body::empty())
        .unwrap();
    let resp = test_app().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn v1_requires_bearer() {
    let no_auth = Request::builder()
        .uri("/v1/orgs")
        .body(Body::empty())
        .unwrap();
    assert_eq!(status_of(no_auth).await, StatusCode::UNAUTHORIZED);

    let wrong = Request::builder()
        .uri("/v1/orgs")
        .header(header::AUTHORIZATION, "Bearer nope")
        .body(Body::empty())
        .unwrap();
    assert_eq!(status_of(wrong).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn operator_lists_orgs() {
    let req = Request::builder()
        .uri("/v1/orgs")
        .header(header::AUTHORIZATION, "Bearer op-secret")
        .body(Body::empty())
        .unwrap();
    let resp = test_app().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body["orgs"].is_array());
}
