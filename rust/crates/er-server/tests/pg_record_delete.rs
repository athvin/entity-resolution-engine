//! Record Delete (spec 2.9) end to end through the production path, gated on
//! BOTH `ER_TEST_PG_DSN` and `DEDUPE_E2E_DUCKLAKE=1`: stage `delete_record` via
//! `POST /records/delete`, drain via `executor_harness`, and confirm the record
//! leaves its entity WITHOUT the singleton identity a suppress keeps; the
//! cell-data lane stops serving it; undelete returns it. And the protect
//! interaction, twice: the endpoint 409s a protected record, and a delete
//! staged straight into the queue (bypassing that precheck) still loses to an
//! active protect at apply time.
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='...' DEDUPE_E2E_DUCKLAKE=1 \
//!     cargo test -p er-server --test main pg_record_delete

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

/// The member record_keys of one entity (empty if the entity is absent → 404).
async fn members(app: &axum::Router, entity_id: &str) -> Vec<String> {
    let resp = app
        .clone()
        .oneshot(op(
            "GET",
            &format!("/v1/orgs/acme/datasets/default/golden-records/{entity_id}"),
            Body::empty(),
        ))
        .await
        .unwrap();
    if resp.status() != StatusCode::OK {
        return Vec::new();
    }
    json_body(resp).await["members"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r["record_key"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// entity_id of a record_key in the current membership (None = placed nowhere).
fn entity_of(session: &dedupe_core::Session, record_key: &str) -> Option<String> {
    session
        .query_strings(&format!(
            "select entity_id from {} where record_key = '{}'",
            er_cli::ENTITY_MEMBERSHIP,
            record_key.replace('\'', "''")
        ))
        .unwrap()
        .into_iter()
        .next()
        .map(|r| r[0].clone())
}

/// The record_keys the cell-data lane serves for the asked keys.
async fn served_keys(app: &axum::Router, keys: &[&str]) -> Vec<String> {
    let query = keys
        .iter()
        .map(|k| format!("key={k}"))
        .collect::<Vec<_>>()
        .join("&");
    let resp = app
        .clone()
        .oneshot(op(
            "GET",
            &format!("/v1/orgs/acme/datasets/default/records?{query}"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    json_body(resp).await["items"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r["record_key"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

async fn delete_removes_a_record_and_undelete_restores_it() {
    let Some(db) = common::fresh_db("record_delete").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping delete e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("er_delete_{}", std::process::id()));
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

    // Find a multi-member entity and pick one of its members to delete.
    let resp = app
        .clone()
        .oneshot(op(
            "GET",
            "/v1/orgs/acme/datasets/default/duplicates",
            Body::empty(),
        ))
        .await
        .unwrap();
    let body = json_body(resp).await;
    let group = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["members"].as_array().is_some_and(|m| m.len() >= 2))
        .expect("base_10 has a multi-member entity");
    let entity_id = group["entity_id"].as_str().unwrap().to_string();
    let before: Vec<String> = group["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap().to_string())
        .collect();
    let victim = before[0].clone();

    // The cell-data lane serves the record before the delete.
    assert!(
        served_keys(&app, &[&victim]).await.contains(&victim),
        "the record's cells are served before the delete"
    );

    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/datasets/default/records/delete",
            Body::from(serde_json::json!({ "record_key": victim }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let org_id = oid(&client, "acme").await;
    let ds = common::default_dataset(&client, "acme").await;
    let mut harness = executor_harness::Harness::start(&dsn, tmp.clone()).await;
    harness.apply_corrections(&client, &org_id, &ds).await;

    // The entity keeps exactly the other members...
    let after = members(&app, &entity_id).await;
    assert!(
        !after.contains(&victim),
        "deleted record {victim} must be gone from entity {entity_id} membership"
    );
    assert_eq!(
        after.len(),
        before.len() - 1,
        "exactly the deleted record left the entity"
    );
    // ...and unlike a suppress, NO singleton identity is minted for it...
    {
        let session = er_cli::open_lake(&catalog, &data_path).unwrap();
        assert_eq!(
            entity_of(&session, &victim),
            None,
            "a deleted record is placed nowhere — suppress keeps a singleton, delete does not"
        );
    }
    // ...and the cell-data lane stops serving it.
    assert!(
        !served_keys(&app, &[&victim]).await.contains(&victim),
        "a deleted record's cells are no longer served"
    );

    // The departure is on the entity's event timeline as `departed`, the same
    // ledger line a source tombstone writes.
    let resp = app
        .clone()
        .oneshot(op(
            "GET",
            &format!("/v1/orgs/acme/datasets/default/golden-records/{entity_id}"),
            Body::empty(),
        ))
        .await
        .unwrap();
    let detail = json_body(resp).await;
    assert!(
        detail["events"].as_array().unwrap().iter().any(|e| {
            e["event_type"] == "member_removed" && e["details"].to_string().contains("departed")
        }),
        "the delete is a member_removed/departed event on the entity"
    );

    // Undelete → the record re-clusters into the corpus (the scores that put
    // it there never changed).
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/datasets/default/records/undelete",
            Body::from(serde_json::json!({ "record_key": victim }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    harness.apply_corrections(&client, &org_id, &ds).await;
    {
        let session = er_cli::open_lake(&catalog, &data_path).unwrap();
        assert!(
            entity_of(&session, &victim).is_some(),
            "undelete places {victim} back into an entity"
        );
    }

    // The delete is audited.
    let resp = app
        .clone()
        .oneshot(op("GET", "/v1/orgs/acme/audit", Body::empty()))
        .await
        .unwrap();
    let audit = json_body(resp).await;
    assert!(
        audit
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["action"] == "steward.delete_record"),
        "delete_record is on the audit ledger"
    );
}

async fn delete_never_touches_a_protected_record() {
    let Some(db) = common::fresh_db("record_delete_protect").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping delete/protect e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("er_delete_prot_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();
    let config_path = tmp.join("acme.yaml").to_string_lossy().into_owned();
    std::fs::write(&config_path, TEST_YAML).unwrap();
    let lexicon_path = tmp.join("nickname_variants.csv");
    std::fs::write(&lexicon_path, NICKNAMES).unwrap();
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

    let resp = app
        .clone()
        .oneshot(op(
            "GET",
            "/v1/orgs/acme/datasets/default/duplicates",
            Body::empty(),
        ))
        .await
        .unwrap();
    let body = json_body(resp).await;
    let group = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["members"].as_array().is_some_and(|m| m.len() >= 2))
        .expect("base_10 has a multi-member entity");
    let entity_id = group["entity_id"].as_str().unwrap().to_string();
    let record = group["members"].as_array().unwrap()[0]
        .as_str()
        .unwrap()
        .to_string();

    // Protect the record, drain so the overlay is live...
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/datasets/default/records/protect",
            Body::from(serde_json::json!({ "record_key": record }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let org_id = oid(&client, "acme").await;
    let ds = common::default_dataset(&client, "acme").await;
    let mut harness = executor_harness::Harness::start(&dsn, tmp.clone()).await;
    harness.apply_corrections(&client, &org_id, &ds).await;

    // ...then the endpoint refuses the delete with a 409 naming the record.
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/datasets/default/records/delete",
            Body::from(serde_json::json!({ "record_key": record }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let detail = json_body(resp).await;
    assert!(
        detail["detail"].as_str().unwrap().contains(&record),
        "the 409 names the protected record"
    );

    let other = group["members"].as_array().unwrap()[1].as_str().unwrap();
    let response = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/datasets/default/merges",
            Body::from(
                serde_json::json!({"master": other, "records": [record, other]}).to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::CONFLICT,
        "protected non-master must survive a mass merge"
    );

    // And a delete staged STRAIGHT into the queue — bypassing the endpoint's
    // precheck, e.g. staged before the protect landed — still loses to the
    // active protect at apply time.
    pg::stage_action(
        &client,
        &org_id,
        Some(&ds),
        &serde_json::json!({
            "type": "delete_record",
            "record_key": record,
            "created_by": "test",
        }),
        "test",
    )
    .await
    .unwrap();
    harness.apply_corrections(&client, &org_id, &ds).await;
    assert!(
        members(&app, &entity_id).await.contains(&record),
        "protect wins: the protected record survives a staged delete"
    );
}

#[tokio::test]
async fn scenarios() {
    delete_removes_a_record_and_undelete_restores_it().await;
    delete_never_touches_a_protected_record().await;
}
