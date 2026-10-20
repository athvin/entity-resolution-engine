//! CSV upload over the real Postgres backend (behind `ER_TEST_PG_DSN`):
//! `POST /v1/orgs/{org}/imports?source=…`.
//!
//! This is the path that the BFF, its mock and the new-source wizard all drove
//! while the server registered only `get(imports_list)` and had no multipart
//! support at all — green in every tier, 405 in a real deployment. These tests
//! exercise the real thing: the delivery lands at `<drop_root>/<source>.csv`,
//! a run is enqueued for it, and each documented refusal is produced rather
//! than assumed. `tests/mock_contract.rs` keeps the mock from drifting ahead of
//! the router again.
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_imports

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

const DEFAULT_YAML: &str = include_str!("../../er-config/tests/fixtures/default.yaml");

/// A multipart body with one `file` part, built by hand so the test does not
/// depend on a client library to spell the wire format.
fn multipart_body(filename: &str, content: &str) -> (String, Body) {
    const BOUNDARY: &str = "----ertestboundary9f3c";
    let body = format!(
        "--{BOUNDARY}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
         Content-Type: text/csv\r\n\r\n\
         {content}\r\n\
         --{BOUNDARY}--\r\n"
    );
    (
        format!("multipart/form-data; boundary={BOUNDARY}"),
        Body::from(body),
    )
}

/// A multipart body whose only part is NOT named `file`.
fn multipart_wrong_part() -> (String, Body) {
    const BOUNDARY: &str = "----ertestboundary9f3c";
    let body = format!(
        "--{BOUNDARY}\r\n\
         Content-Disposition: form-data; name=\"notes\"\r\n\r\n\
         hello\r\n\
         --{BOUNDARY}--\r\n"
    );
    (
        format!("multipart/form-data; boundary={BOUNDARY}"),
        Body::from(body),
    )
}

fn upload_req(uri: &str, bearer: &str, content_type: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(body)
        .unwrap()
}

fn json_req(method: &str, uri: &str, bearer: &str, body: Body) -> Request<Body> {
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
    json_body(resp).await["key"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn csv_upload_lands_a_delivery_and_enqueues_its_run() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres imports e2e (set ER_TEST_PG_DSN to run)");
        return;
    };

    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS staged_steward_actions, audit_log, schedules, config_versions, api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    // A small cap so the oversize refusal is reachable without a 256 MiB body.
    let cap = 1024usize;
    let app =
        app(AppState::with_pg("op-secret".to_string(), dsn.clone()).with_max_import_bytes(cap));

    let tmp = std::env::temp_dir().join(format!("er_imports_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let config_path = tmp.join("config.yaml").to_string_lossy().to_string();
    let drop_root = tmp.join("drop").to_string_lossy().to_string();
    std::fs::create_dir_all(&tmp).unwrap();

    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/v1/orgs",
            "op-secret",
            Body::from(
                serde_json::json!({
                    "name": "acme",
                    "config_path": config_path,
                    "drop_root": drop_root,
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let admin_key = api_key(&app, "admin").await;
    let steward_key = api_key(&app, "steward").await;
    let viewer_key = api_key(&app, "viewer").await;

    // ---- before any publish: there is no active config to validate against ----
    let (ct, body) = multipart_body("crm.csv", "crm_id,first_name\nC1,ada\n");
    let resp = app
        .clone()
        .oneshot(upload_req(
            "/v1/orgs/acme/imports?source=crm",
            &steward_key,
            &ct,
            body,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "an upload before the first publish has no declared sources to check"
    );

    // Publish v1 (the default fixture declares crm / billing / webforms).
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/v1/orgs/acme/config/versions",
            &admin_key,
            Body::from(serde_json::json!({ "yaml": DEFAULT_YAML }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let version = json_body(resp).await["version"].as_i64().unwrap();
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/v1/orgs/acme/config/versions/{version}/publish"),
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // ---- the happy path ----
    let csv = "crm_id,first_name,last_name\nC1,ada,lovelace\nC2,alan,turing\n";
    let (ct, body) = multipart_body("crm.csv", csv);
    let resp = app
        .clone()
        .oneshot(upload_req(
            "/v1/orgs/acme/imports?source=crm",
            &steward_key,
            &ct,
            body,
        ))
        .await
        .unwrap();
    let status = resp.status();
    let accepted = json_body(resp).await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "upload refused: {}",
        accepted["detail"].as_str().unwrap_or("(no detail)")
    );
    let job_id = accepted["job"]["id"].as_str().unwrap().to_string();
    assert_ne!(accepted["delivery_id"].as_str().unwrap(), "");

    // The delivery is where the dispatcher looks for it, byte-for-byte.
    let landed = std::fs::read_to_string(format!("{drop_root}/crm.csv")).unwrap();
    assert_eq!(landed, csv, "the delivery must land verbatim");

    // Nothing half-written is left behind.
    let leftovers: Vec<_> = std::fs::read_dir(&drop_root)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| {
            std::path::Path::new(n)
                .extension()
                .is_some_and(|e| e == "part")
        })
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp parts left behind: {leftovers:?}"
    );

    // A run was enqueued for this delivery, and it names the source.
    let resp = app
        .clone()
        .oneshot(json_req(
            "GET",
            &format!("/v1/orgs/acme/jobs/{job_id}"),
            &steward_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let job = json_body(resp).await;
    assert_eq!(job["kind"], "run_all_incremental");
    assert_eq!(job["params"]["source"], "crm");

    // A second upload of the same source is a second delivery, not a no-op:
    // an idempotency key per source would collapse them forever.
    let (ct, body) = multipart_body("crm.csv", "crm_id,first_name\nC3,grace\n");
    let resp = app
        .clone()
        .oneshot(upload_req(
            "/v1/orgs/acme/imports?source=crm",
            &steward_key,
            &ct,
            body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let second = json_body(resp).await;
    assert_ne!(
        second["job"]["id"].as_str().unwrap(),
        job_id,
        "re-uploading a source must enqueue another run"
    );
    assert_eq!(
        std::fs::read_to_string(format!("{drop_root}/crm.csv")).unwrap(),
        "crm_id,first_name\nC3,grace\n",
        "the newest delivery replaces the previous one"
    );

    // ---- refusals ----

    // A viewer may read receipts but not deliver.
    let (ct, body) = multipart_body("crm.csv", "crm_id\nC9\n");
    let resp = app
        .clone()
        .oneshot(upload_req(
            "/v1/orgs/acme/imports?source=crm",
            &viewer_key,
            &ct,
            body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // No `source` query.
    let (ct, body) = multipart_body("crm.csv", "crm_id\nC9\n");
    let resp = app
        .clone()
        .oneshot(upload_req("/v1/orgs/acme/imports", &steward_key, &ct, body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // A source the active config does not declare — and the refusal names the
    // ones it does, because that is what the operator needs to fix it.
    let (ct, body) = multipart_body("x.csv", "a\n1\n");
    let resp = app
        .clone()
        .oneshot(upload_req(
            "/v1/orgs/acme/imports?source=nosuch",
            &steward_key,
            &ct,
            body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let detail = json_body(resp).await["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        detail.contains("crm"),
        "refusal should name known sources: {detail}"
    );

    // A body with no part named `file`.
    let (ct, body) = multipart_wrong_part();
    let resp = app
        .clone()
        .oneshot(upload_req(
            "/v1/orgs/acme/imports?source=crm",
            &steward_key,
            &ct,
            body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Over the cap.
    let (ct, body) = multipart_body("big.csv", &"x".repeat(cap + 1));
    let resp = app
        .clone()
        .oneshot(upload_req(
            "/v1/orgs/acme/imports?source=crm",
            &steward_key,
            &ct,
            body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        std::fs::read_to_string(format!("{drop_root}/crm.csv")).unwrap(),
        "crm_id,first_name\nC3,grace\n",
        "a rejected oversize upload must not disturb the landed delivery"
    );

    // A suspended org stops accepting deliveries.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/v1/orgs/acme/suspend",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let (ct, body) = multipart_body("crm.csv", "crm_id\nC9\n");
    let resp = app
        .clone()
        .oneshot(upload_req(
            "/v1/orgs/acme/imports?source=crm",
            &steward_key,
            &ct,
            body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    let _ = std::fs::remove_dir_all(&tmp);
}
