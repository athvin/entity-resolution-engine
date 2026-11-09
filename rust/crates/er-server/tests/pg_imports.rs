//! Getting data in, over the real Postgres backend (behind `ER_TEST_PG_DSN`):
//! `POST /v1/orgs/{org}/datasets/default/imports?source=…` (a file) and
//! `POST /v1/orgs/{org}/datasets/default/records:ingest` (JSON).
//!
//! Both land the same way — rendered into the source's declared CSV shape,
//! staged under a temp name, renamed into the drop root, run enqueued — because
//! a second ingest path would be a second set of semantics to keep honest
//! against the first.
//!
//! This is the path that the BFF, its mock and the new-source wizard all drove
//! while the server registered only `get(imports_list)` and had no multipart
//! support at all — green in every tier, 405 in a real deployment. These tests
//! exercise the real thing: each delivery gets an immutable file and manifest,
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

use crate::common;

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
async fn deliveries_arrive_as_a_file_or_as_json_and_enqueue_their_run() {
    let Some(db) = common::fresh_db("imports").await else {
        return;
    };
    let dsn = db.dsn().to_string();

    let mut client = pg::connect(&dsn).await.unwrap();
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
            "/v1/orgs/acme/datasets/default/imports?source=crm",
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
            "/v1/orgs/acme/datasets/default/config/versions",
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
            &format!("/v1/orgs/acme/datasets/default/config/versions/{version}/publish"),
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
            "/v1/orgs/acme/datasets/default/imports?source=crm",
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

    // Accept B before A's consumer starts. A must still resolve its own bytes.
    let csv_b = "crm_id,first_name,last_name\nC3,grace,hopper\n";
    let (ct, body) = multipart_body("crm.csv", csv_b);
    let response_b = app
        .clone()
        .oneshot(upload_req(
            "/v1/orgs/acme/datasets/default/imports?source=crm",
            &steward_key,
            &ct,
            body,
        ))
        .await
        .unwrap();
    assert_eq!(response_b.status(), StatusCode::ACCEPTED);
    let accepted_b = json_body(response_b).await;
    let model_path = tmp.join("resolver-model.json");
    std::fs::write(&model_path, "{}").unwrap();
    let resolver_env = serde_json::json!({
        "ER_CONFIG_PATH": config_path,
        "ER_MODEL_PATH": model_path,
        "ER_TF_PATH": tmp.join("tf.csv"),
    });
    for (delivery, expected) in [(&accepted, csv), (&accepted_b, csv_b), (&accepted, csv)] {
        let resolved = er_server::runctx::RunContext::resolve(&er_server::runctx::ResolveRequest {
            lake_env: &resolver_env,
            params: &delivery["job"]["params"],
            models: &[],
            drop_root: Some(&drop_root),
            wants_deliveries: true,
            dataset_id: delivery["job"]["dataset_id"].as_str(),
        })
        .unwrap();
        assert_eq!(
            resolved.deliveries,
            vec![("crm".to_string(), expected.to_string())],
            "each queued delivery, including a retry, must retain its exact bytes"
        );
    }

    // The delivery is where the dispatcher looks for it, byte-for-byte.
    let landed = std::fs::read_to_string(format!(
        "{drop_root}/deliveries/{}.csv",
        accepted["delivery_id"].as_str().unwrap()
    ))
    .unwrap();
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
            &format!("/v1/orgs/acme/datasets/default/jobs/{job_id}"),
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
            "/v1/orgs/acme/datasets/default/imports?source=crm",
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
        std::fs::read_to_string(format!(
            "{drop_root}/deliveries/{}.csv",
            second["delivery_id"].as_str().unwrap()
        ))
        .unwrap(),
        "crm_id,first_name\nC3,grace\n",
        "each delivery has its own immutable bytes"
    );

    // ---- refusals ----

    // A viewer may read receipts but not deliver.
    let (ct, body) = multipart_body("crm.csv", "crm_id\nC9\n");
    let resp = app
        .clone()
        .oneshot(upload_req(
            "/v1/orgs/acme/datasets/default/imports?source=crm",
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
        .oneshot(upload_req(
            "/v1/orgs/acme/datasets/default/imports",
            &steward_key,
            &ct,
            body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // A source the active config does not declare — and the refusal names the
    // ones it does, because that is what the operator needs to fix it.
    let (ct, body) = multipart_body("x.csv", "a\n1\n");
    let resp = app
        .clone()
        .oneshot(upload_req(
            "/v1/orgs/acme/datasets/default/imports?source=nosuch",
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
            "/v1/orgs/acme/datasets/default/imports?source=crm",
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
            "/v1/orgs/acme/datasets/default/imports?source=crm",
            &steward_key,
            &ct,
            body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        std::fs::read_to_string(format!(
            "{drop_root}/deliveries/{}.csv",
            second["delivery_id"].as_str().unwrap()
        ))
        .unwrap(),
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
            "/v1/orgs/acme/datasets/default/imports?source=crm",
            &steward_key,
            &ct,
            body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // ---- the JSON push: the write half of the batch API ----
    //
    // Before this, data could only arrive as a file. A pipeline had no way to
    // hand records over programmatically, which is the integration a data
    // service is asked for first. It deliberately takes the same path a file
    // does — rendered into the source's declared CSV shape, staged, run
    // enqueued — rather than a second ingest path to keep honest.
    let resume = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/v1/orgs/acme/resume",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(
        resume.status(),
        StatusCode::OK,
        "reactivate for the push tests"
    );

    // A viewer cannot push.
    let push = serde_json::json!({
        "source": "crm",
        "records": [{ "crm_id": "P1", "first_name": "ada", "last_name": "lovelace" }],
    });
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/v1/orgs/acme/datasets/default/records/ingest",
            &viewer_key,
            Body::from(push.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // The happy path.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/v1/orgs/acme/datasets/default/records/ingest",
            &steward_key,
            Body::from(push.to_string()),
        ))
        .await
        .unwrap();
    let status = resp.status();
    let pushed = json_body(resp).await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "push refused: {}",
        pushed["detail"].as_str().unwrap_or("(no detail)")
    );
    assert_ne!(pushed["delivery_id"].as_str().unwrap(), "");
    assert_eq!(pushed["job"]["kind"], "run_all_incremental");
    assert_eq!(pushed["job"]["params"]["source"], "crm");

    // The delivery landed as the source's declared CSV shape — a header of the
    // DECLARED columns, not of whatever keys the record happened to carry, so a
    // record that omits a column lands as an empty cell rather than shifting
    // every later one.
    let landed = std::fs::read_to_string(format!(
        "{drop_root}/deliveries/{}.csv",
        pushed["delivery_id"].as_str().unwrap()
    ))
    .unwrap();
    let mut lines = landed.lines();
    let header: Vec<&str> = lines.next().unwrap().split(',').collect();
    assert!(
        header.contains(&"crm_id")
            && header.contains(&"first_name")
            && header.contains(&"email_address"),
        "the header must be the source's declared columns: {header:?}"
    );
    let row: Vec<&str> = lines.next().unwrap().split(',').collect();
    assert_eq!(
        row.len(),
        header.len(),
        "every row matches the header width"
    );
    let cell = |name: &str| row[header.iter().position(|h| *h == name).unwrap()];
    assert_eq!(cell("crm_id"), "P1");
    assert_eq!(cell("first_name"), "ada");
    assert_eq!(
        cell("email_address"),
        "",
        "an omitted column lands empty, not shifted"
    );
    assert_eq!(lines.next(), None, "one record in, one row out");

    // A value with a comma must not break the CSV — the failure mode that would
    // silently corrupt every column after it.
    let tricky = serde_json::json!({
        "source": "crm",
        "records": [{
            "crm_id": "P2",
            "last_name": "O\"Hara, Jr",
            "first_name": "line\nbreak",
        }],
    });
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/v1/orgs/acme/datasets/default/records/ingest",
            &steward_key,
            Body::from(tricky.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let tricky_delivery = json_body(resp).await;
    let landed = std::fs::read_to_string(format!(
        "{drop_root}/deliveries/{}.csv",
        tricky_delivery["delivery_id"].as_str().unwrap()
    ))
    .unwrap();
    assert!(
        landed.contains("\"O\"\"Hara, Jr\""),
        "a comma and a quote must be CSV-escaped: {landed}"
    );

    // An undeclared column is REPORTED, not dropped: silently ignoring it would
    // mean a caller believing it sent data the pipeline never saw.
    let unknown = serde_json::json!({
        "source": "crm",
        "records": [{ "crm_id": "P3", "favourite_colour": "blue" }],
    });
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/v1/orgs/acme/datasets/default/records/ingest",
            &steward_key,
            Body::from(unknown.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let detail = json_body(resp).await["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        detail.contains("favourite_colour"),
        "the refusal must name the offending column: {detail}"
    );

    // Empty, non-object and undeclared-source bodies are refused.
    for (body, expected) in [
        (
            serde_json::json!({ "source": "crm", "records": [] }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            serde_json::json!({ "source": "crm", "records": ["not an object"] }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            serde_json::json!({ "source": "nosuch", "records": [{ "crm_id": "P4" }] }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/v1/orgs/acme/datasets/default/records/ingest",
                &steward_key,
                Body::from(body.to_string()),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), expected, "for body {body}");
    }

    let _ = std::fs::remove_dir_all(&tmp);
}
