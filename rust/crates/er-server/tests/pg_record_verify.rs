//! Persisted row and cell verification through HTTP, the executor and read filters.
use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

use crate::common;
use crate::executor_harness;

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

/// The org's id for a test org name.
async fn oid(client: &tokio_postgres::Client, name: &str) -> String {
    er_server::pg::org_id_by_name(client, name)
        .await
        .expect("org lookup")
        .expect("org exists")
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
        mode: "full",
        rebuild_reason: None,
        segment: None,
        lexicon: &lexicon,
        ignore_words: &[],
        deliveries: &deliveries,
        model: &model,
        run_id: "01DELETERUN0000000000000000",
        ingested_at: "2026-01-01 00:00:00",
        scored_at: "2026-01-02 00:00:00",
        run_at: "2026-01-03 00:00:00",
    };
    er_cli::run_all(&session, &config, &inputs).unwrap();
}

#[tokio::test]
async fn verification_persists_and_retracts_independently() {
    let Some(db) = common::fresh_db("record_verify").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping delete e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("er_verify_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();
    let config_path = tmp.join("acme.yaml").to_string_lossy().into_owned();
    std::fs::write(&config_path, TEST_YAML).unwrap();
    let lexicon_path = tmp.join("nickname_variants.csv");
    std::fs::write(&lexicon_path, NICKNAMES).unwrap();
    // A COPY of the TF fixture, never the fixture itself: a snapshot APPENDS to
    // that file, so pointing a tenant at a committed one has the test mutate
    // the repo.
    let tf_path = tmp.join("model_test_v1.tf.csv");
    std::fs::copy(repo_fixture("model_test_v1.tf.csv"), &tf_path).unwrap();
    populate_lake(&catalog, &data_path);

    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let env = serde_json::json!({
        "ER_LAKE_CATALOG": catalog,
        "ER_LAKE_DATA_PATH": data_path,
        "ER_CONFIG_PATH": config_path,
        "ER_LEXICON_PATH": lexicon_path.to_string_lossy(),
        "ER_MODEL_PATH": repo_fixture("model_test_v1.json"),
        "ER_TF_PATH": tf_path.to_string_lossy(),
        "ER_MODEL_VERSION": "v0001",
        "ER_TF_SNAPSHOT_ID": "01JWMDTFSNAP00000000000001",
    });
    let create = serde_json::json!({ "name": "acme", "config_path": config_path, "env": env });
    assert_eq!(
        app.clone()
            .oneshot(op("POST", "/v1/orgs", Body::from(create.to_string())))
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );

    let response = app
        .clone()
        .oneshot(op(
            "GET",
            "/v1/orgs/acme/datasets/default/duplicates",
            Body::empty(),
        ))
        .await
        .unwrap();
    let groups = json_body(response).await;
    let group = &groups["items"][0];
    let record = group["members"][0].as_str().unwrap();
    let entity = group["entity_id"].as_str().unwrap();
    let org_id = oid(&client, "acme").await;
    let ds = common::default_dataset(&client, "acme").await;
    let mut harness = executor_harness::Harness::start(&dsn, tmp.clone()).await;
    for (action, attribute) in [
        ("verify", Some("given_name")),
        ("verify", None),
        ("unverify", None),
        ("unverify", Some("given_name")),
    ] {
        let mut payload = serde_json::json!({"record_key": record});
        if let Some(attribute) = attribute {
            payload["attribute"] = attribute.into();
        }
        let response = app
            .clone()
            .oneshot(op(
                "POST",
                &format!("/v1/orgs/acme/datasets/default/records/{action}"),
                Body::from(payload.to_string()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        harness.apply_corrections(&client, &org_id, &ds).await;
        let response = app
            .clone()
            .oneshot(op(
                "GET",
                &format!("/v1/orgs/acme/datasets/default/records?key={record}"),
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let values = json_body(response).await;
        let row = &values["items"][0];
        assert_eq!(row["verified"], action == "verify" && attribute.is_none());
        let cells = row["verified_attributes"].as_array().unwrap();
        assert_eq!(
            cells.contains(&serde_json::json!("given_name")),
            !(action == "unverify" && attribute.is_some())
        );
        let response = app
            .clone()
            .oneshot(op(
                "GET",
                &format!("/v1/orgs/acme/datasets/default/golden-records/{entity}"),
                Body::empty(),
            ))
            .await
            .unwrap();
        let detail = json_body(response).await;
        let member = detail["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["record_key"] == record)
            .unwrap();
        assert_eq!(member["verified"], row["verified"]);
        let response = app
            .clone()
            .oneshot(op(
                "GET",
                "/v1/orgs/acme/datasets/default/duplicates?verified=true",
                Body::empty(),
            ))
            .await
            .unwrap();
        let filtered = json_body(response).await;
        assert_eq!(
            filtered["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|g| g["entity_id"] == entity),
            action == "verify" && attribute.is_none()
        );
    }
}
