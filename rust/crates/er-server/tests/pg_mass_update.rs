//! Mass-update (golden field overrides) end to end through the production path:
//! stage override/clear actions via `POST /mass-update`, drain via
//! `dispatcher::drain_and_correct`, and read the golden record back — the
//! override value lands (and a clear nulls the field). Gated on BOTH
//! `ER_TEST_PG_DSN` and `DEDUPE_E2E_DUCKLAKE=1`. Its own binary (tests in one
//! binary race on the shared control-plane schema).

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, dispatcher, pg};
use tower::ServiceExt;

const TEST_YAML: &str = include_str!("../../er-cli/tests/fixtures/test.yaml");
const NICKNAMES: &str = include_str!("../../er-cli/tests/fixtures/nickname_variants.csv");

fn repo_fixture(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/static")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

fn op(method: &str, uri: &str, body: Body) -> Request<Body> {
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

fn populate_lake(catalog: &str, data_path: &str) {
    let session = er_cli::open_lake(catalog, data_path).unwrap();
    let config = er_config::load_config_str(TEST_YAML).unwrap();
    let lexicon: Vec<(String, String)> = NICKNAMES
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (a, b) = l.split_once(',').unwrap();
            (a.to_string(), b.to_string())
        })
        .collect();
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
    let model = er_cli::MatchModel {
        settings_json: &model_json,
        tf_csv_path: std::path::Path::new(&tf_csv),
        model_version: "v0001",
        tf_snapshot_id: "01JWMDTFSNAP00000000000001",
    };
    let inputs = er_cli::PipelineInputs {
        lexicon: &lexicon,
        deliveries: &deliveries,
        model: &model,
        run_id: "01MASSUPDRUN0000000000000000",
        ingested_at: "2026-01-01 00:00:00",
        scored_at: "2026-01-02 00:00:00",
        run_at: "2026-01-03 00:00:00",
    };
    er_cli::run_all(&session, &config, &inputs).unwrap();
}

async fn golden_email(app: &axum::Router, entity_id: &str) -> Option<String> {
    let resp = app
        .clone()
        .oneshot(op("GET", "/v1/orgs/acme/golden-records", Body::empty()))
        .await
        .unwrap();
    let items = json_body(resp).await;
    items["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["entity_id"] == entity_id)
        .map(|r| r["email"].clone())
        .and_then(|v| {
            if v.is_null() {
                None
            } else {
                Some(v.as_str().unwrap().to_string())
            }
        })
}

#[tokio::test]
async fn mass_update_overrides_and_clears_golden_fields() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping mass-update e2e (set ER_TEST_PG_DSN to run)");
        return;
    };
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping mass-update e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("er_massupd_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();
    let config_path = tmp.join("acme.yaml").to_string_lossy().into_owned();
    std::fs::write(&config_path, TEST_YAML).unwrap();
    populate_lake(&catalog, &data_path);

    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS staged_steward_actions, audit_log, schedules, config_versions, \
             api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let env = serde_json::json!({
        "ER_LAKE_CATALOG": catalog,
        "ER_LAKE_DATA_PATH": data_path,
        "ER_CONFIG_PATH": config_path,
    });
    let create = serde_json::json!({ "name": "acme", "config_path": config_path, "env": env });
    let resp = app
        .clone()
        .oneshot(op("POST", "/v1/orgs", Body::from(create.to_string())))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Pick an entity to edit.
    let resp = app
        .clone()
        .oneshot(op("GET", "/v1/orgs/acme/golden-records", Body::empty()))
        .await
        .unwrap();
    let entity_id = json_body(resp).await["items"][0]["entity_id"]
        .as_str()
        .unwrap()
        .to_string();

    // 1) Override `email` for that entity, then drain + correct.
    let body = serde_json::json!({
        "items": [{ "entity_id": entity_id, "attribute": "email", "value": "override@dupezero.test" }]
    });
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/mass-update",
            Body::from(body.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(pg::pending_count(&client, "acme").await.unwrap(), 1);
    dispatcher::drain_and_correct(&client, "acme", &env, "01MUCORRECT00000000000000001")
        .await
        .unwrap();
    assert_eq!(
        golden_email(&app, &entity_id).await.as_deref(),
        Some("override@dupezero.test"),
        "the override value lands on the golden record"
    );

    // 2) Clear `email` for that entity, then drain + correct → NULL.
    let body = serde_json::json!({
        "items": [{ "entity_id": entity_id, "attribute": "email", "clear": true }]
    });
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/mass-update",
            Body::from(body.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    dispatcher::drain_and_correct(&client, "acme", &env, "01MUCORRECT00000000000000002")
        .await
        .unwrap();
    assert_eq!(
        golden_email(&app, &entity_id).await,
        None,
        "clear_field nulls the golden field"
    );

    // 3) Validation: an unknown attribute is rejected.
    let bad = serde_json::json!({
        "items": [{ "entity_id": entity_id, "attribute": "not_a_field", "value": "x" }]
    });
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/mass-update",
            Body::from(bad.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // 4) The edits are audited.
    let resp = app
        .oneshot(op("GET", "/v1/orgs/acme/audit", Body::empty()))
        .await
        .unwrap();
    let audit = json_body(resp).await;
    assert!(
        audit
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["action"] == "steward.mass_update"),
        "mass_update is on the audit ledger"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
