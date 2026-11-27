//! #994: delegated Salesforce authorization for writeback, against a mock
//! Salesforce. The core flow — code exchange with PKCE, identity binding,
//! refresh, revocation, denial, transient failure and a token minted for
//! another instance — and the routes' pre-network guarantees: redirect
//! allowlist, roles, single-use state bound to org, connection and the
//! starting principal, and no token ever in a response. The worker helper
//! refuses unattended use unless allowed and revokes a grant Salesforce has
//! withdrawn.
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Form, State as AxState};
use axum::http::{Request, StatusCode, header};
use axum::routing::{get, post};
use er_server::sf_oauth::{self, AccessError, OAuthError};
use er_server::urlguard::Egress;
use er_server::{AppState, app, pg};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::common;

/// The mock's view of the world: which refresh tokens are live, and whether
/// the identity service lies about the org.
#[derive(Default)]
struct Mock {
    base: String,
    lie_about_org: bool,
}

async fn token(
    AxState(m): AxState<Arc<Mutex<Mock>>>,
    Form(form): Form<std::collections::HashMap<String, String>>,
) -> (StatusCode, axum::Json<Value>) {
    let base = m.lock().unwrap().base.clone();
    let ok = |refresh: Option<&str>| {
        (
            StatusCode::OK,
            axum::Json(json!({
                "access_token": "at-1", "refresh_token": refresh, "instance_url": base,
                "id": format!("{base}/id/00Dorg/005user"), "scope": "api refresh_token",
            })),
        )
    };
    match (
        form.get("grant_type").map(String::as_str),
        form.get("code").map(String::as_str),
    ) {
        (Some("authorization_code"), Some("good"))
            if form.get("code_verifier").is_some_and(|v| v == "v-good") =>
        {
            ok(Some("r-good"))
        }
        (Some("authorization_code"), Some("elsewhere")) => (
            StatusCode::OK,
            axum::Json(
                json!({"access_token": "at", "refresh_token": "r", "instance_url": "https://other.my.salesforce.com", "id": "https://other.my.salesforce.com/id/00Dx/005x"}),
            ),
        ),
        (Some("authorization_code"), _) => (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": "invalid_grant", "error_description": "secret detail"})),
        ),
        (Some("refresh_token"), _) => match form.get("refresh_token").map(String::as_str) {
            Some("r-good") => ok(None),
            Some("r-flaky") => (StatusCode::SERVICE_UNAVAILABLE, axum::Json(json!({}))),
            Some("r-denied") => (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({"error": "invalid_client"})),
            ),
            _ => (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({"error": "invalid_grant"})),
            ),
        },
        _ => (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": "unsupported_grant_type"})),
        ),
    }
}

async fn identity(AxState(m): AxState<Arc<Mutex<Mock>>>) -> axum::Json<Value> {
    let lie = m.lock().unwrap().lie_about_org;
    axum::Json(json!({
        "organization_id": if lie { "00Dother" } else { "00Dorg" },
        "user_id": "005user", "username": "reviewer@acme.test",
    }))
}

async fn call(
    router: &axum::Router,
    method: &str,
    uri: &str,
    key: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn delegated_authorization_binds_the_reviewer_and_fails_closed() {
    // --- The mock Salesforce ---------------------------------------------------
    let mock = Arc::new(Mutex::new(Mock::default()));
    let sf = axum::Router::new()
        .route("/services/oauth2/token", post(token))
        .route("/services/oauth2/revoke", post(|| async { StatusCode::OK }))
        .route("/id/00Dorg/005user", get(identity))
        .with_state(Arc::clone(&mock));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    mock.lock().unwrap().base = base.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, sf).await;
    });

    // --- 1. The core flow --------------------------------------------------
    let tokens = sf_oauth::exchange_code(
        &base,
        "cid",
        "sec",
        "good",
        "v-good",
        "https://app/cb",
        Egress::Unguarded,
    )
    .await
    .unwrap();
    assert_eq!(tokens.refresh_token.as_deref(), Some("r-good"));
    let who = sf_oauth::identity(&tokens, Egress::Unguarded)
        .await
        .unwrap();
    assert_eq!(
        (who.org_id.as_str(), who.user_id.as_str()),
        ("00Dorg", "005user")
    );
    let wrong = sf_oauth::exchange_code(
        &base,
        "cid",
        "sec",
        "good",
        "v-wrong",
        "https://app/cb",
        Egress::Unguarded,
    )
    .await;
    assert!(
        matches!(wrong, Err(OAuthError::Revoked(_))),
        "a wrong PKCE verifier is refused"
    );
    let elsewhere = sf_oauth::exchange_code(
        &base,
        "cid",
        "sec",
        "elsewhere",
        "v-good",
        "https://app/cb",
        Egress::Unguarded,
    )
    .await;
    assert!(
        matches!(elsewhere, Err(OAuthError::OrgMismatch)),
        "tokens for another instance are refused"
    );
    mock.lock().unwrap().lie_about_org = true;
    assert!(matches!(
        sf_oauth::identity(&tokens, Egress::Unguarded).await,
        Err(OAuthError::OrgMismatch)
    ));
    mock.lock().unwrap().lie_about_org = false;
    for (refresh, check) in [
        ("r-good", "ok"),
        ("r-flaky", "transient"),
        ("r-denied", "denied"),
        ("r-gone", "revoked"),
    ] {
        let got = sf_oauth::refresh(&base, "cid", "sec", refresh, Egress::Unguarded).await;
        let kind = match &got {
            Ok(_) => "ok",
            Err(OAuthError::Transient(_)) => "transient",
            Err(OAuthError::Denied(_)) => "denied",
            Err(OAuthError::Revoked(_)) => "revoked",
            Err(_) => "other",
        };
        assert_eq!(kind, check, "{refresh}");
    }
    let leaked = sf_oauth::exchange_code(
        &base,
        "cid",
        "sec",
        "bad",
        "v",
        "https://app/cb",
        Egress::Unguarded,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(!leaked.contains("secret detail"), "{leaked}");

    // --- 2. The routes --------------------------------------------------------
    let Some(db) = common::fresh_db("sf_delegated_auth").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let unconfigured = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let router = app(AppState::with_pg("op-secret".to_string(), dsn.clone())
        .with_salesforce_redirects(vec!["https://app.test/cb".to_string()]));
    let mut keys = std::collections::BTreeMap::new();
    for org in ["acme", "globex"] {
        call(
            &router,
            "POST",
            "/v1/orgs",
            "op-secret",
            Some(json!({"name": org, "config_path": format!("/tmp/{org}-oauth.yaml")})),
        )
        .await;
        for role in ["admin", "viewer"] {
            for n in 0..2 {
                let (_, k) = call(
                    &router,
                    "POST",
                    &format!("/v1/orgs/{org}/api-keys"),
                    "op-secret",
                    Some(json!({"role": role})),
                )
                .await;
                keys.insert(
                    format!("{org}:{role}:{n}"),
                    k["key"].as_str().unwrap().to_owned(),
                );
            }
        }
    }
    let org = pg::get_org(&client, "acme").await.unwrap().unwrap();
    // A real instance host, so the start route's guard accepts it.
    let conn = pg::create_salesforce_connection(
        &client,
        &org.id,
        "prod",
        "https://acme.my.salesforce.com",
        "client_credentials",
        "cid",
        "sec",
        "v62.0",
    )
    .await
    .unwrap();
    let path = format!("/v1/orgs/acme/connections/{conn}/writeback-authorization");
    let start = json!({"redirect_uri": "https://app.test/cb"});

    let (status, _) = call(
        &unconfigured,
        "POST",
        &path,
        &keys["acme:admin:0"],
        Some(start.clone()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "no redirect configured, no flow"
    );
    let (status, _) = call(
        &router,
        "POST",
        &path,
        &keys["acme:viewer:0"],
        Some(start.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &router,
        "POST",
        &path,
        &keys["acme:admin:0"],
        Some(json!({"redirect_uri": "https://evil.test/cb"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "redirect must be allowlisted"
    );
    let (status, _) = call(
        &router,
        "POST",
        &format!("/v1/orgs/globex/connections/{conn}/writeback-authorization"),
        &keys["globex:admin:0"],
        Some(start.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "another org's connection");
    let (status, started) = call(
        &router,
        "POST",
        &path,
        &keys["acme:admin:0"],
        Some(start.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{started}");
    let url = reqwest::Url::parse(started["authorize_url"].as_str().unwrap()).unwrap();
    let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["redirect_uri"], "https://app.test/cb");
    let state = q["state"].clone();

    let complete = format!("{path}/complete");
    let (status, _) = call(
        &router,
        "POST",
        &complete,
        &keys["acme:admin:0"],
        Some(json!({"state": "forged", "code": "good"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "unknown state");
    let (status, _) = call(
        &router,
        "POST",
        &complete,
        &keys["acme:admin:1"],
        Some(json!({"state": state, "code": "good"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "another admin cannot complete someone else's flow"
    );
    let (status, _) = call(
        &router,
        "POST",
        &complete,
        &keys["acme:admin:0"],
        Some(json!({"state": state, "code": "good"})),
    )
    .await;
    assert_ne!(
        status,
        StatusCode::OK,
        "acme.my.salesforce.com is not reachable from the test, so the exchange fails"
    );
    let (status, used) = call(
        &router,
        "POST",
        &complete,
        &keys["acme:admin:0"],
        Some(json!({"state": state, "code": "good"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a state is single-use: {used}"
    );
    let (status, _) = call(&router, "GET", &path, &keys["acme:viewer:0"], None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no grant yet");

    // --- 3. The worker's token, against the mock --------------------------------
    let mock_conn = pg::create_salesforce_connection(
        &client,
        &org.id,
        "mock",
        &base,
        "client_credentials",
        "cid",
        "sec",
        "v62.0",
    )
    .await
    .unwrap();
    for (refresh, automatic) in [("r-good", false)] {
        er_entities::salesforce_delegated_grant::create::grant(
            &client,
            &er_ids::new_v7(),
            &org.id,
            &mock_conn,
            "00Dorg",
            "005user",
            Some("reviewer@acme.test"),
            refresh,
            "api refresh_token",
            automatic,
            "key:admin",
        )
        .await
        .unwrap();
    }
    let access = sf_oauth::access_token(&client, &org.id, &mock_conn, false, Egress::Unguarded)
        .await
        .unwrap();
    assert_eq!(
        (access.sf_org_id.as_str(), access.sf_user_id.as_str()),
        ("00Dorg", "005user")
    );
    assert!(
        matches!(
            sf_oauth::access_token(&client, &org.id, &mock_conn, true, Egress::Unguarded).await,
            Err(AccessError::NotForAutomatic)
        ),
        "unattended work needs explicit authorization"
    );
    let mock_path = format!("/v1/orgs/acme/connections/{mock_conn}/writeback-authorization");
    let (status, view) = call(&router, "GET", &mock_path, &keys["acme:viewer:0"], None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["status"], "active");
    assert!(
        !view.to_string().contains("r-good"),
        "no token in a response: {view}"
    );
    // Consent withdrawn at Salesforce: the next refresh revokes the grant here.
    er_entities::salesforce_delegated_grant::update::revoke(
        &client, "replaced", &org.id, &mock_conn,
    )
    .await
    .unwrap();
    er_entities::salesforce_delegated_grant::create::grant(
        &client,
        &er_ids::new_v7(),
        &org.id,
        &mock_conn,
        "00Dorg",
        "005user",
        None,
        "r-gone",
        "api refresh_token",
        true,
        "key:admin",
    )
    .await
    .unwrap();
    assert!(matches!(
        sf_oauth::access_token(&client, &org.id, &mock_conn, true, Egress::Unguarded).await,
        Err(AccessError::OAuth(OAuthError::Revoked(_)))
    ));
    let (_, view) = call(&router, "GET", &mock_path, &keys["acme:viewer:0"], None).await;
    assert_eq!(
        (view["status"].clone(), view["revoked_reason"].clone()),
        (json!("revoked"), json!("consent_withdrawn")),
        "{view}"
    );
    assert!(
        matches!(
            sf_oauth::access_token(&client, &org.id, &mock_conn, false, Egress::Unguarded).await,
            Err(AccessError::NotAuthorized)
        ),
        "a revoked grant authorizes nothing, and client credentials never stand in"
    );
    let (status, _) = call(&router, "DELETE", &mock_path, &keys["acme:viewer:0"], None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
