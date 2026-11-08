//! Salesforce connections over the real Postgres backend (behind
//! `ER_TEST_PG_DSN`): register (the secret is accepted and never shown again,
//! not even in the 201), list (secret withheld), retire (soft, idempotent),
//! the outbound-URL refusals, the role gates, the audit row's discipline, and
//! the extract endpoint's 422s-before-queueing. One test, sequential phases —
//! the pg_* convention.
//!
//! What is deliberately NOT here: the extract DAG running against a mock
//! Salesforce (the Bulk client itself is proven in `salesforce_mock.rs`), and
//! the work-order resolution — both need the executor harness and are the
//! follow-up named in the PR.
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_connections

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

use crate::common;

const DEFAULT_YAML: &str = include_str!("../../er-config/tests/fixtures/default.yaml");

/// A value that must never appear in any response body after the create.
const SECRET: &str = "SECRET-SENTINEL-3f9c";

fn json_req(method: &str, uri: &str, bearer: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .unwrap()
}

async fn body_text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Mint an org key of `role` for `acme`.
async fn api_key(app: &axum::Router, role: &str) -> String {
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/v1/orgs/acme/api-keys",
            "op-secret",
            Body::from(format!(r#"{{"role":"{role}"}}"#)),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    serde_json::from_str::<serde_json::Value>(&body_text(resp).await).unwrap()["key"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn connections_crud_and_extract_preconditions() {
    let Some(db) = common::fresh_db("connections").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    let tmp = std::env::temp_dir().join(format!("er_connections_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("drop")).unwrap();
    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/v1/orgs",
            "op-secret",
            Body::from(
                serde_json::json!({
                    "name": "acme",
                    "config_path": tmp.join("config.yaml").to_string_lossy(),
                    "drop_root": tmp.join("drop").to_string_lossy(),
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let admin = api_key(&app, "admin").await;
    let steward = api_key(&app, "steward").await;
    let viewer = api_key(&app, "viewer").await;

    let good = serde_json::json!({
        "name": "prod",
        "instance_url": "https://acme.my.salesforce.example",
        "client_id": "consumer-key",
        "client_secret": SECRET,
    })
    .to_string();

    // ---- the role gates: registering a credential is admin's, like webhooks ----
    for (key, who) in [(&steward, "steward"), (&viewer, "viewer")] {
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/v1/orgs/acme/connections",
                key,
                Body::from(good.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{who} must not mint");
    }

    // ---- the outbound-URL guard, observed refusing on the real path ----
    for (url, why) in [
        ("http://acme.my.salesforce.example", "https only"),
        (
            "https://169.254.169.254/latest/meta-data/",
            "instance metadata",
        ),
        ("https://127.0.0.1/", "loopback"),
    ] {
        let body = serde_json::json!({
            "name": "bad",
            "instance_url": url,
            "client_id": "k",
            "client_secret": "s",
        })
        .to_string();
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/v1/orgs/acme/connections",
                &admin,
                Body::from(body),
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{why}: {url}"
        );
    }

    // ---- the shape refusals ----
    for (field, value) in [("auth_kind", "web_server"), ("api_version", "62")] {
        let mut body: serde_json::Value = serde_json::from_str(&good).unwrap();
        body[field] = serde_json::json!(value);
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/v1/orgs/acme/connections",
                &admin,
                Body::from(body.to_string()),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY, "{field}");
    }

    // ---- create: the 201 echoes the non-secret half ONLY ----
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/v1/orgs/acme/connections",
            &admin,
            Body::from(good.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let text = body_text(resp).await;
    assert!(
        !text.contains(SECRET) && !text.contains("client_secret"),
        "the 201 must not carry the secret: {text}"
    );
    let created: serde_json::Value = serde_json::from_str(&text).unwrap();
    let connection_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["auth_kind"], "client_credentials");
    assert_eq!(created["api_version"], "v62.0");

    // ---- a duplicate name is a conflict, not a twin ----
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/v1/orgs/acme/connections",
            &admin,
            Body::from(good.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // ---- list: viewer may read, and the secret is not in the body ----
    let resp = app
        .clone()
        .oneshot(json_req(
            "GET",
            "/v1/orgs/acme/connections",
            &viewer,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = body_text(resp).await;
    assert!(
        !text.contains(SECRET) && !text.contains("client_secret"),
        "the listing must not carry the secret: {text}"
    );
    let listed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1, "one connection listed");
    assert_eq!(listed[0]["name"], "prod");

    // ---- the audit row names the connection, never the credential ----
    let row = client
        .query_one(
            "SELECT detail::text FROM public.audit_log WHERE action = 'connection.create'",
            &[],
        )
        .await
        .expect("the create wrote its audit row");
    let detail: String = row.get(0);
    assert!(detail.contains(&connection_id), "{detail}");
    assert!(
        !detail.contains(SECRET) && !detail.contains("client_secret"),
        "the audit detail must not carry the secret: {detail}"
    );

    // ---- extract: every refusal arrives at request time, not in the queue ----
    let extract = |body: serde_json::Value, key: &str| {
        json_req(
            "POST",
            "/v1/orgs/acme/datasets/default/extract",
            key,
            Body::from(body.to_string()),
        )
    };
    // A malformed request names its parameter.
    let resp = app
        .clone()
        .oneshot(extract(
            serde_json::json!({ "connection_id": &connection_id, "source": "crm" }),
            &steward,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body_text(resp).await.contains("object"));
    // A dangling connection is the caller's typo.
    let resp = app
        .clone()
        .oneshot(extract(
            serde_json::json!({
                "connection_id": "nope", "object": "Contact", "source": "crm",
            }),
            &steward,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    // No published config yet: the delivery would be refused at ingest, so the
    // request is refused now.
    let resp = app
        .clone()
        .oneshot(extract(
            serde_json::json!({
                "connection_id": &connection_id, "object": "Contact", "source": "crm",
            }),
            &steward,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body_text(resp).await.contains("no published config"));

    // Publish the default fixture (sources crm / billing / webforms).
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/v1/orgs/acme/datasets/default/config/versions",
            &admin,
            Body::from(serde_json::json!({ "yaml": DEFAULT_YAML }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let version =
        serde_json::from_str::<serde_json::Value>(&body_text(resp).await).unwrap()["version"]
            .as_i64()
            .unwrap();
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/v1/orgs/acme/datasets/default/config/versions/{version}/publish"),
            &admin,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // An undeclared source names the declared ones.
    let resp = app
        .clone()
        .oneshot(extract(
            serde_json::json!({
                "connection_id": &connection_id, "object": "Lead", "source": "sf_lead",
            }),
            &steward,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body_text(resp).await.contains("crm"));

    // The happy path queues an extract job.
    let resp = app
        .clone()
        .oneshot(extract(
            serde_json::json!({
                "connection_id": &connection_id, "object": "Contact", "source": "crm",
            }),
            &steward,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let queued: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert_eq!(queued["job"]["kind"], "extract");
    assert_eq!(queued["job"]["state"], "queued");

    // A viewer may not start one.
    let resp = app
        .clone()
        .oneshot(extract(
            serde_json::json!({
                "connection_id": &connection_id, "object": "Contact", "source": "crm",
            }),
            &viewer,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // ---- schedules: `extract` is schedulable, and its params are validated ----
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/v1/orgs/acme/datasets/default/schedules",
            &admin,
            Body::from(serde_json::json!({ "kind": "extract", "cron": "0 * * * *" }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "an extract schedule with empty params would enqueue failures forever"
    );
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/v1/orgs/acme/datasets/default/schedules",
            &admin,
            Body::from(
                serde_json::json!({
                    "kind": "extract",
                    "cron": "0 * * * *",
                    "params": {
                        "connection_id": &connection_id, "object": "Contact", "source": "crm",
                    },
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // ---- retire: soft and idempotent, and the listing forgets it ----
    let resp = app
        .clone()
        .oneshot(json_req(
            "DELETE",
            &format!("/v1/orgs/acme/connections/{connection_id}"),
            &admin,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let first: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert_eq!(first["deleted"], true);
    let resp = app
        .clone()
        .oneshot(json_req(
            "DELETE",
            &format!("/v1/orgs/acme/connections/{connection_id}"),
            &admin,
            Body::empty(),
        ))
        .await
        .unwrap();
    let second: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert_eq!(second["deleted"], false, "a second retire changed nothing");
    let resp = app
        .clone()
        .oneshot(json_req(
            "GET",
            "/v1/orgs/acme/connections",
            &viewer,
            Body::empty(),
        ))
        .await
        .unwrap();
    let listed: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert!(
        listed.as_array().unwrap().is_empty(),
        "retired = not listed"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
