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
    let job_id = job["id"].as_str().unwrap().to_string();
    assert_eq!(job["state"], "queued");
    assert_eq!(job["kind"], "run");
    assert_eq!(job["org_id"], "acme");

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

#[tokio::test]
async fn action_colon_spelling_routes_like_the_subpath() {
    // The BFF addresses actions as `{id}:action` (Python convention); the server
    // normalizes that to the `{id}/action` sub-path it routes on.
    let app = test_app();

    let resp = app
        .clone()
        .oneshot(op_request(
            "POST",
            "/v1/orgs/acme/jobs",
            Body::from(r#"{"kind":"run"}"#),
        ))
        .await
        .unwrap();
    let job_id = json_body(resp).await["id"].as_str().unwrap().to_string();

    // Cancel via `{id}:cancel` (NOT the `/cancel` sub-path).
    let resp = app
        .clone()
        .oneshot(op_request(
            "POST",
            &format!("/v1/orgs/acme/jobs/{job_id}:cancel"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["state"], "canceled");

    // `{org}:suspend` normalizes too — reaches the suspend handler, which in the
    // in-memory (no-Postgres) backend reports 501 rather than 404 (the point is
    // that it ROUTED, not that it succeeded without a DB).
    let resp = app
        .oneshot(op_request("POST", "/v1/orgs/acme:suspend", Body::empty()))
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "{{org}}:suspend must route to the suspend handler"
    );
}

// --- POST /v1/email ---------------------------------------------------------
//
// The BFF's only mail path. These lock the contract `relayEmail` depends on:
// it treats a non-2xx as `false` and falls back, so the distinction that
// matters is 422 (the caller's bug, visible in review) versus 503 (this
// environment simply has no mail backend, which is normal in dev).

fn email_body(template: &str, params: serde_json::Value) -> Body {
    Body::from(
        serde_json::json!({ "to": "person@example.com", "template": template, "params": params })
            .to_string(),
    )
}

fn invite_params() -> serde_json::Value {
    serde_json::json!({
        "org": "acme",
        "role": "steward",
        "invited_by": "admin@acme.com",
        "link": "https://app.example.com/invite/tok",
    })
}

#[tokio::test]
async fn email_is_operator_only() {
    // No credential at all.
    let anon = Request::builder()
        .method("POST")
        .uri("/v1/email")
        .header(header::CONTENT_TYPE, "application/json")
        .body(email_body("invite", invite_params()))
        .unwrap();
    assert_eq!(
        test_app().oneshot(anon).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    // A bearer that is not the operator token. Nothing short of the operator
    // may make this service send mail to an arbitrary address.
    let not_operator = Request::builder()
        .method("POST")
        .uri("/v1/email")
        .header(header::AUTHORIZATION, "Bearer erk_someone_elses")
        .header(header::CONTENT_TYPE, "application/json")
        .body(email_body("invite", invite_params()))
        .unwrap();
    assert_ne!(
        test_app().oneshot(not_operator).await.unwrap().status(),
        StatusCode::ACCEPTED,
        "a non-operator credential must not be able to send mail"
    );
}

#[tokio::test]
async fn email_without_a_backend_is_503_not_a_silent_success() {
    // AppState::new defaults to Mailer::off() — the state of every environment
    // that has not configured mail, including the whole repo before this route
    // existed. 503 is what relayEmail turns into `false`.
    let resp = test_app()
        .oneshot(op_request(
            "POST",
            "/v1/email",
            email_body("invite", invite_params()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        json_body(resp).await["detail"]
            .as_str()
            .unwrap()
            .contains("not configured")
    );
}

#[tokio::test]
async fn a_caller_side_mistake_is_422_in_every_environment() {
    // Rendering runs before the backend is consulted, so a bad template is
    // reported as the caller's bug even where mail is switched off. Otherwise a
    // typo would be invisible until someone enabled a backend.
    let resp = test_app()
        .oneshot(op_request(
            "POST",
            "/v1/email",
            email_body("welcome_aboard", serde_json::json!({})),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        json_body(resp).await["detail"]
            .as_str()
            .unwrap()
            .contains("welcome_aboard")
    );

    // A known template missing the one parameter that carries its payload.
    let resp = test_app()
        .oneshot(op_request(
            "POST",
            "/v1/email",
            email_body("password_reset", serde_json::json!({})),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        json_body(resp).await["detail"]
            .as_str()
            .unwrap()
            .contains("link")
    );

    // An empty recipient.
    let resp = test_app()
        .oneshot(op_request(
            "POST",
            "/v1/email",
            Body::from(
                serde_json::json!({ "to": "  ", "template": "password_reset",
                                    "params": { "link": "https://x/y" } })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn email_with_a_backend_is_accepted() {
    let app =
        app(AppState::new("op-secret".to_string()).with_mail(er_server::mail::Mailer::logging()));
    for (template, params) in [
        ("invite", invite_params()),
        (
            "password_reset",
            serde_json::json!({ "link": "https://app.example.com/reset/tok" }),
        ),
        (
            "signup_verify",
            serde_json::json!({ "workspace": "acme",
                                "link": "https://app.example.com/signup/verify/tok" }),
        ),
    ] {
        let resp = app
            .clone()
            .oneshot(op_request(
                "POST",
                "/v1/email",
                email_body(template, params),
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::ACCEPTED,
            "{template} must be accepted"
        );
    }
}
