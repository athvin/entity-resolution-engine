//! Dataset CRUD over the real Postgres backend (behind `ER_TEST_PG_DSN`):
//! bring-your-own create with its validation edges (reserved names, entity
//! types, the config_path/env.ER_CONFIG_PATH one-source-of-truth rule, the
//! 409 on a name collision, the 409 on a suspended org), list/detail (and
//! that neither leaks the lake env or config path), the display-name PATCH,
//! the suspend/resume state machine, and the role boundary — all through the
//! HTTP surface. Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test main pg_datasets
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).

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

#[tokio::test]
async fn dataset_crud_validation_and_state_machine() {
    let Some(db) = common::fresh_db("datasets").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    // ---- an org to put datasets in ----------------------------------------
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

    // ---- create: BYO, 201, and the row is exactly what was sent -----------
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/datasets",
            "op-secret",
            Body::from(
                r#"{"name":"crm","display_name":"CRM contacts","entity_type":"account",
                    "config_path":"/configs/acme/crm.yaml",
                    "env":{"ER_LAKE_CATALOG":"postgres:dbname=er_t_acme",
                           "ER_LAKE_CATALOG_SCHEMA":"ds_crm",
                           "ER_LAKE_DATA_PATH":"/lake/acme/crm"}}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let org_id = pg::org_id_by_name(&client, "acme").await.unwrap().unwrap();
    let row = pg::get_dataset(&client, &org_id, "crm")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.entity_type, "account");
    assert_eq!(row.state, "active");
    assert_eq!(row.config_path, "/configs/acme/crm.yaml");
    // config_path was mirrored into the env (the one-source-of-truth rule).
    assert_eq!(
        row.env.get("ER_CONFIG_PATH").and_then(|v| v.as_str()),
        Some("/configs/acme/crm.yaml")
    );
    assert_eq!(
        row.env
            .get("ER_LAKE_CATALOG_SCHEMA")
            .and_then(|v| v.as_str()),
        Some("ds_crm")
    );

    // ---- create: the validation edges --------------------------------------
    // Name collision: 409, and the existing row is not refreshed.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/datasets",
            "op-secret",
            Body::from(r#"{"name":"crm","config_path":"/elsewhere.yaml"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let row = pg::get_dataset(&client, &org_id, "crm")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.config_path, "/configs/acme/crm.yaml",
        "409 means untouched"
    );

    // Reserved names collide with the frontend's static segments.
    for reserved in ["datasets", "settings"] {
        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                "/v1/orgs/acme/datasets",
                "op-secret",
                Body::from(format!(
                    r#"{{"name":"{reserved}","config_path":"/x.yaml"}}"#
                )),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // A bad entity type, a bad slug, a config_path/env mismatch: all 422.
    for body in [
        r#"{"name":"bad1","entity_type":"ticket","config_path":"/x.yaml"}"#,
        r#"{"name":"Bad Slug","config_path":"/x.yaml"}"#,
        r#"{"name":"bad2","config_path":"/x.yaml","env":{"ER_CONFIG_PATH":"/other.yaml"}}"#,
    ] {
        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                "/v1/orgs/acme/datasets",
                "op-secret",
                Body::from(body),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }

    // No config_path: the honest 503 until dataset auto-provisioning exists.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/datasets",
            "op-secret",
            Body::from(r#"{"name":"auto1"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

    // ---- read: list + detail, and what they deliberately do NOT say -------
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/datasets",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let list = json_body(resp).await;
    let names: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["name"].as_str())
        .collect();
    assert!(
        names.contains(&"crm"),
        "list carries the dataset: {names:?}"
    );

    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/datasets/crm",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let detail = json_body(resp).await;
    assert_eq!(detail["entity_type"], "account");
    assert_eq!(detail["display_name"], "CRM contacts");
    // The lake env and config path are plumbing, not product state.
    assert!(detail.get("env").is_none(), "env must not leak: {detail}");
    assert!(
        detail.get("config_path").is_none(),
        "config_path must not leak: {detail}"
    );

    // Unknown dataset: the scope-safe 404.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/datasets/nope",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // ---- update: display label only ----------------------------------------
    let resp = app
        .clone()
        .oneshot(req(
            "PATCH",
            "/v1/orgs/acme/datasets/crm",
            "op-secret",
            Body::from(r#"{"display_name":"CRM (renamed)"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["display_name"], "CRM (renamed)");

    // ---- lifecycle: suspend / resume with the 409 edges ---------------------
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/datasets/crm/suspend",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["state"], "suspended");
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/datasets/crm/suspend",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT, "already suspended");
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/datasets/crm/resume",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // ---- the role boundary --------------------------------------------------
    // Mint a viewer and an admin key for acme.
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
    assert_eq!(resp.status(), StatusCode::CREATED);
    let viewer_key = json_body(resp).await["key"].as_str().unwrap().to_string();
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/api-keys",
            "op-secret",
            Body::from(r#"{"role":"admin"}"#),
        ))
        .await
        .unwrap();
    let admin_key = json_body(resp).await["key"].as_str().unwrap().to_string();

    // A viewer reads but neither creates (operator-only) nor suspends (admin).
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/datasets",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/datasets",
            &viewer_key,
            Body::from(r#"{"name":"nope","config_path":"/x.yaml"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/datasets/crm/suspend",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    // An admin runs the lifecycle.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/datasets/crm/suspend",
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // ---- a suspended ORG refuses dataset creation ---------------------------
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
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/datasets",
            "op-secret",
            Body::from(r#"{"name":"late","config_path":"/x.yaml"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}
