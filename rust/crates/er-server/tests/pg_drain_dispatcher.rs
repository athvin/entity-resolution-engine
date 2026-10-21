//! The steward drain driven through the PRODUCTION path
//! `dispatcher::drain_and_correct` (what an `apply_corrections` job runs), rather
//! than a hand-rolled apply loop — the regression guard for the all-Rust
//! rebuild's unported `steward.drain_org`. Also exercises the `run_correct` that
//! follows the drain, so the org env carries `ER_CONFIG_PATH`.
//!
//! Its own test binary (not a second test in `pg_drain.rs`): tests within one
//! binary run on parallel threads, and this + `pg_drain` both DROP/recreate the
//! shared control-plane schema and create org `acme`, so they must not run
//! concurrently. Gated on BOTH `ER_TEST_PG_DSN` and `DEDUPE_E2E_DUCKLAKE=1`.

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, dispatcher, pg};
use tower::ServiceExt;

/// The org's id for a test org name (child wrappers key on org_id).
async fn oid(client: &tokio_postgres::Client, name: &str) -> String {
    er_server::pg::org_id_by_name(client, name)
        .await
        .expect("org lookup")
        .expect("org exists")
}

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
        mode: "full",
        rebuild_reason: None,
        segment: None,
        lexicon: &lexicon,
        deliveries: &deliveries,
        model: &model,
        run_id: "01DRAINRUN00000000000000000",
        ingested_at: "2026-01-01 00:00:00",
        scored_at: "2026-01-02 00:00:00",
        run_at: "2026-01-03 00:00:00",
    };
    er_cli::run_all(&session, &config, &inputs).unwrap();
}

#[tokio::test]
async fn drain_and_correct_applies_through_dispatcher() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping drain-dispatcher e2e (set ER_TEST_PG_DSN to run)");
        return;
    };
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping drain-dispatcher e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("er_drain_disp_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();
    // run_correct (which follows the drain) loads the config from ER_CONFIG_PATH.
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
    let create = serde_json::json!({
        "name": "acme",
        "config_path": config_path,
        "env": env,
    });
    let resp = app
        .clone()
        .oneshot(op("POST", "/v1/orgs", Body::from(create.to_string())))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(op("GET", "/v1/orgs/acme/reviews", Body::empty()))
        .await
        .unwrap();
    let review_id = json_body(resp).await["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Stage an assertion + a review resolution (stage only; drain is separate).
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/assertions",
            Body::from(r#"{"kind":"always","a":"crm:C001","b":"billing:B001"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            &format!("/v1/orgs/acme/reviews/{review_id}/resolve"),
            Body::from(r#"{"resolution":"match"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        pg::pending_count(&client, &oid(&client, "acme").await)
            .await
            .unwrap(),
        2
    );

    // Drive the real drain path (what an `apply_corrections` job runs).
    dispatcher::drain_and_correct(
        &client,
        &oid(&client, "acme").await,
        &env,
        "01DISPCORRECT0000000000000001",
    )
    .await
    .unwrap();

    // Everything applied + marked.
    assert_eq!(
        pg::pending_count(&client, &oid(&client, "acme").await)
            .await
            .unwrap(),
        0
    );
    let resp = app
        .clone()
        .oneshot(op("GET", "/v1/orgs/acme/assertions", Body::empty()))
        .await
        .unwrap();
    let rows = json_body(resp).await["items"].as_array().unwrap().clone();
    assert_eq!(rows.len(), 2, "add_assertion + match-derived assertion");
    assert!(rows.iter().all(|r| r["active"] == true));

    // The resolution survived the run_correct that followed the drain.
    let resp = app
        .oneshot(op(
            "GET",
            &format!("/v1/orgs/acme/reviews/{review_id}"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["status"], "resolved_match");

    let _ = std::fs::remove_dir_all(&tmp);
}
