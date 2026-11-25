//! #1230: the export routes through the router, over a real lake and Postgres
//! (behind `ER_TEST_PG_DSN` and `DEDUPE_E2E_DUCKLAKE=1`).
//!
//! The pod task is `export_write.rs`'s subject; this file drives the job row
//! through its states by hand, because running the real task needs the
//! dispatcher and a pod. What it proves is the API's half:
//!
//! - `POST` pins a snapshot and answers it, refuses a bad combination (422) and
//!   a viewer (403), and writes an `export.create` audit row;
//! - `GET` status reads the job back, including its pinned snapshot;
//! - download is a 409 until the export succeeds, streams the artifact with
//!   its media type once it has, and is a 410 once seven days have passed;
//! - another dataset's export and another org's are both 404, never a leak.

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, exports, pg};
use tower::ServiceExt;

use crate::common;

const TEST_YAML: &str = include_str!("../../er-cli/tests/fixtures/test.yaml");

fn repo_fixture(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/static")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

fn call(method: &str, uri: &str, bearer: &str, body: Option<serde_json::Value>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap()
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// base_10's resolved results in a fresh lake at `(catalog, data)`.
fn populate_lake(catalog: &str, data_path: &str) {
    let session = er_cli::open_lake(catalog, data_path).unwrap();
    let config = er_config::load_config_str(TEST_YAML).unwrap();
    let deliveries: Vec<(String, String)> = ["crm", "billing", "webforms"]
        .iter()
        .map(|s| {
            (
                (*s).to_string(),
                std::fs::read_to_string(repo_fixture(&format!("base_10/base/{s}.csv"))).unwrap(),
            )
        })
        .collect();
    let model_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let tf_csv = repo_fixture("model_test_v1.tf.csv");
    er_cli::run_all(
        &session,
        &config,
        &er_cli::PipelineInputs {
            mode: "full",
            rebuild_reason: None,
            segment: None,
            lexicon: &Vec::new(),
            ignore_words: &[],
            deliveries: &deliveries,
            model: &er_cli::MatchModel {
                settings_json: &model_json,
                tf_csv_path: std::path::Path::new(&tf_csv),
                model_version: "v0001",
                tf_snapshot_id: "01JWMDTFSNAP00000000000001",
            },
            run_id: "01EXPORTROUTE0000000000000",
            ingested_at: "2026-01-01 00:00:00",
            scored_at: "2026-01-02 00:00:00",
            run_at: "2026-01-03 00:00:00",
        },
    )
    .unwrap();
}

#[tokio::test]
async fn the_export_routes_pin_serve_and_expire() {
    let Some(db) = common::fresh_db("exports").await else {
        return;
    };
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping export routes e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let dsn = db.dsn().to_string();
    let tmp = std::env::temp_dir().join(format!("er_pg_exports_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("lake/data")).unwrap();
    let catalog = tmp
        .join("lake/catalog.ducklake")
        .to_string_lossy()
        .into_owned();
    let data_path = tmp.join("lake/data").to_string_lossy().into_owned();
    let config_path = tmp.join("config.yaml");
    std::fs::write(&config_path, TEST_YAML).unwrap();
    let config_path = config_path.to_string_lossy().into_owned();
    populate_lake(&catalog, &data_path);

    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    for name in ["acme", "other"] {
        let resp = app
            .clone()
            .oneshot(call(
                "POST",
                "/v1/orgs",
                "op-secret",
                Some(serde_json::json!({
                    "name": name,
                    "config_path": config_path,
                    "env": { "ER_LAKE_CATALOG": catalog, "ER_LAKE_DATA_PATH": data_path, "ER_CONFIG_PATH": config_path },
                })),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED, "create org {name}");
    }
    // A second dataset in acme, over the same lake, to prove an export is
    // scoped to its dataset and not merely to its org.
    let acme = pg::get_org(&client, "acme").await.unwrap().unwrap();
    pg::create_dataset_byo(
        &client,
        &acme.id,
        "second",
        None,
        "person",
        &config_path,
        &serde_json::json!({ "ER_LAKE_CATALOG": catalog, "ER_LAKE_DATA_PATH": data_path, "ER_CONFIG_PATH": config_path }),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let mint = |org: &'static str, role: &'static str| {
        let app = app.clone();
        async move {
            let resp = app
                .oneshot(call(
                    "POST",
                    &format!("/v1/orgs/{org}/api-keys"),
                    "op-secret",
                    Some(serde_json::json!({ "role": role })),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::CREATED);
            json_body(resp).await["key"].as_str().unwrap().to_string()
        }
    };
    let steward = mint("acme", "steward").await;
    let viewer = mint("acme", "viewer").await;
    let outsider = mint("other", "admin").await;
    let base = "/v1/orgs/acme/datasets/default/exports";

    // POST: refused for a viewer and for an impossible combination.
    let resp = app
        .clone()
        .oneshot(call(
            "POST",
            base,
            &viewer,
            Some(serde_json::json!({"kind": "golden", "format": "csv"})),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let resp = app
        .clone()
        .oneshot(call(
            "POST",
            base,
            &steward,
            Some(serde_json::json!({"kind": "duplicates", "format": "csv"})),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // POST: accepted, the latest snapshot pinned and answered.
    let latest = {
        let session = er_server::read::open_org_lake(&serde_json::json!({
            "ER_LAKE_CATALOG": catalog, "ER_LAKE_DATA_PATH": data_path,
        }))
        .unwrap();
        er_server::read::latest_snapshot(&session).unwrap()
    };
    let resp = app
        .clone()
        .oneshot(call(
            "POST",
            base,
            &steward,
            Some(serde_json::json!({"kind": "golden", "format": "csv"})),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let queued = json_body(resp).await;
    let export_id = queued["export_id"].as_str().unwrap().to_string();
    assert_eq!(queued["snapshot"], latest, "{queued}");
    assert_eq!(queued["state"], "queued");
    let audit = pg::audit_rows(
        &client,
        Some("acme"),
        &pg::AuditFilters::default(),
        None,
        50,
    )
    .await
    .unwrap();
    assert!(
        audit
            .iter()
            .any(|e| e.action == "export.create" && e.detail["export_id"] == export_id.as_str()),
        "{audit:?}"
    );

    // GET status, and a download refused until it succeeds.
    let status_uri = format!("{base}/{export_id}");
    let download_uri = format!("{status_uri}/download");
    let resp = app
        .clone()
        .oneshot(call("GET", &status_uri, &viewer, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let status = json_body(resp).await;
    assert_eq!(status["state"], "queued");
    assert_eq!(status["snapshot"], latest);
    assert_eq!(status["kind"], "golden");
    let resp = app
        .clone()
        .oneshot(call("GET", &download_uri, &viewer, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // Another dataset's route and another org's key: 404, never the export.
    for (uri, bearer) in [
        (
            format!("/v1/orgs/acme/datasets/second/exports/{export_id}"),
            viewer.as_str(),
        ),
        (status_uri.clone(), outsider.as_str()),
        (
            format!("/v1/orgs/other/datasets/default/exports/{export_id}"),
            outsider.as_str(),
        ),
    ] {
        let resp = app
            .clone()
            .oneshot(call("GET", &uri, bearer, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{uri}");
    }

    // Succeed it: the artifact the pod task would write, then the job row.
    let request = exports::ExportRequest::from_params(
        &serde_json::json!({"kind": "golden", "format": "csv", "snapshot": latest}),
    )
    .unwrap();
    let path = exports::artifact_path(
        &serde_json::json!({ "ER_LAKE_DATA_PATH": data_path }),
        &export_id,
        request.format,
    )
    .unwrap();
    {
        let session = er_server::read::open_org_lake(&serde_json::json!({
            "ER_LAKE_CATALOG": catalog, "ER_LAKE_DATA_PATH": data_path,
        }))
        .unwrap();
        let report = exports::write(
            &session,
            er_config::EntityType::Person,
            &request,
            &path,
            200,
            exports::MAX_ROWS,
        )
        .unwrap();
        assert_eq!(report.rows, 10, "base_10 resolves to 10 golden records");
    }
    client
        .execute(
            "UPDATE jobs SET state = 'succeeded', finished_at = now() WHERE id = $1",
            &[&export_id],
        )
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(call("GET", &download_uri, &viewer, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()[header::CONTENT_TYPE],
        "text/csv; charset=utf-8"
    );
    assert!(
        resp.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains(&format!("{export_id}.csv"))
    );
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
        .await
        .unwrap();
    let csv = String::from_utf8(bytes.to_vec()).unwrap();
    assert_eq!(csv.lines().count(), 11, "a header and ten rows:\n{csv}");
    let resp = app
        .clone()
        .oneshot(call("GET", &status_uri, &viewer, None))
        .await
        .unwrap();
    let status = json_body(resp).await;
    assert_eq!(status["state"], "succeeded");
    assert!(status["expires_at"].is_string(), "{status}");

    // Past retention: 410, even with the file still on disk.
    client
        .execute(
            "UPDATE jobs SET finished_at = now() - interval '8 days' WHERE id = $1",
            &[&export_id],
        )
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(call("GET", &download_uri, &viewer, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::GONE);

    let _ = std::fs::remove_dir_all(&tmp);
}
