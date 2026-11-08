//! The credential filter over the real Postgres backend (behind
//! `ER_TEST_PG_DSN`): nothing credential-shaped persists into an org or
//! dataset row (#792). The write is refused before it reaches the database —
//! a 422 naming the key (never a value) — at both the org upsert and the
//! dataset create, and a refused write creates nothing. Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test main pg_env_filter

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

use crate::common;

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

/// The credential filter (#792): nothing credential-shaped persists into an
/// org or dataset row. The write is refused before it reaches the database —
/// a 422 naming the key (never a value) — at both the org upsert and the
/// dataset create, and a refused write creates nothing.
#[tokio::test]
async fn credential_keys_are_refused_in_env_writes() {
    let Some(db) = common::fresh_db("credfilter").await else {
        return;
    };
    let dsn = db.dsn().to_string();

    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    // Org create: each credential-shaped name is a 422 naming the key, and
    // creates nothing.
    for key in [
        "ER_LAKE_S3_SECRET",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_ACCESS_KEY_ID",
    ] {
        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                "/v1/orgs",
                "op-secret",
                Body::from(format!(
                    r#"{{"name":"creds","config_path":"/configs/creds.yaml","env":{{"{key}":"x"}}}}"#
                )),
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{key} must be refused"
        );
        let detail = json_body(resp).await["detail"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            detail.contains(key),
            "the refusal names the key, got {detail:?}"
        );
    }
    let resp = app
        .clone()
        .oneshot(req("GET", "/v1/orgs/creds", "op-secret", Body::empty()))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "a refused env write creates no org"
    );

    // A clean org, then a dataset create carrying a credential key: 422, and
    // no dataset.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs",
            "op-secret",
            Body::from(r#"{"name":"acme","config_path":"/configs/acme.yaml"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/datasets",
            "op-secret",
            Body::from(
                r#"{"name":"d1","config_path":"/configs/d1.yaml","env":{"ER_LAKE_S3_KEY_ID":"minio"}}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let detail = json_body(resp).await["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(detail.contains("ER_LAKE_S3_KEY_ID"), "{detail:?}");
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/datasets/d1",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "a refused env write creates no dataset"
    );
}
