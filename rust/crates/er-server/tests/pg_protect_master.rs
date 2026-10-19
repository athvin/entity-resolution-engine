//! Protect enforcement end to end through the production path: stage
//! `protect_record` via `POST /records/protect`, drain via
//! `dispatcher::drain_and_correct` (which writes the `protect` overlay), and
//! confirm the protected member becomes the elected master for its entity on the
//! `GET /merge-plans` read path; retract via `/records/unprotect` + drain and the
//! policy master returns. Gated on BOTH `ER_TEST_PG_DSN` and
//! `DEDUPE_E2E_DUCKLAKE=1`. Own binary (tests in one binary race on the shared
//! control-plane schema).

use std::path::PathBuf;

use arrow::array::{Array, StringArray};
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

/// The golden-records list data lane is Arrow IPC; decode the page's `entity_id`
/// column into a plain `Vec<String>`.
async fn golden_entity_ids(app: &axum::Router) -> Vec<String> {
    let resp = app
        .clone()
        .oneshot(op("GET", "/v1/orgs/acme/golden-records", Body::empty()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let mut ids = Vec::new();
    for batch in
        arrow::ipc::reader::StreamReader::try_new(std::io::Cursor::new(bytes.to_vec()), None)
            .unwrap()
            .map(Result::unwrap)
    {
        let idx = batch.schema().index_of("id").unwrap();
        let col = batch
            .column(idx)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        for i in 0..col.len() {
            ids.push(col.value(i).to_string());
        }
    }
    ids
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

/// Build a lake with a REAL merge: resolve `merge_scenario`'s base delivery (two
/// entities), then its incremental batch (a webforms record that bridges them),
/// so reconcile emits the `merged` / `member_added` events that `merge-plans`
/// reads (base_10 only emits `created`, so its plan is always empty). The writer
/// session is dropped before the app/dispatcher reopen the lake.
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
    er_cli::seed_lexicon(&session, &lexicon).unwrap();
    let model_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let tf_csv = repo_fixture("model_test_v1.tf.csv");
    let model = er_cli::MatchModel {
        settings_json: &model_json,
        tf_csv_path: std::path::Path::new(&tf_csv),
        model_version: "v0001",
        tf_snapshot_id: "01JWMDTFSNAP00000000000001",
    };
    for (phase, batch_id, run_id, ts) in [
        (
            "base",
            "01MERGEBASEBATCH000000000001",
            "01MERGEBASERUN00000000000001",
            "2026-01-01 00:00:00",
        ),
        (
            "batch",
            "01MERGEINCRBATCH000000000001",
            "01MERGEINCRRUN00000000000001",
            "2026-01-05 00:00:00",
        ),
    ] {
        // Record the run in the `runs` ledger (write_run_start/end wrap run_all;
        // the granular path must do it too, or run_correct has no prior run).
        let inputs = er_cli::PipelineInputs {
            lexicon: &[],
            deliveries: &[],
            model: &model,
            run_id,
            ingested_at: ts,
            scored_at: ts,
            run_at: ts,
        };
        er_cli::write_run_start(&session, &config, &inputs).unwrap();
        for source in ["crm", "billing", "webforms"] {
            let path = repo_fixture(&format!("merge_scenario/{phase}/{source}.csv"));
            if let Ok(csv) = std::fs::read_to_string(&path) {
                er_cli::run_ingest(&session, &config, source, &csv, batch_id, ts, false).unwrap();
            }
        }
        er_cli::run_standardize(&session, &config, ts).unwrap();
        er_cli::run_match(&session, &config, &model, 0.60, run_id, ts).unwrap();
        er_cli::run_reconcile(&session, &config, run_id, ts).unwrap();
        er_cli::run_golden(&session, &config, ts).unwrap();
        er_cli::write_run_end(&session, run_id, ts).unwrap();
    }
}

/// The member record_keys of one entity (empty if the entity is absent → 404).
async fn members(app: &axum::Router, entity_id: &str) -> Vec<String> {
    let resp = app
        .clone()
        .oneshot(op(
            "GET",
            &format!("/v1/orgs/acme/golden-records/{entity_id}"),
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

/// The elected master_key for `entity_id` from the merge-plans read path (None if
/// the entity has no plan row — e.g. fewer than two members).
async fn plan_master(app: &axum::Router, entity_id: &str) -> Option<String> {
    let resp = app
        .clone()
        .oneshot(op("GET", "/v1/orgs/acme/merge-plans", Body::empty()))
        .await
        .unwrap();
    json_body(resp).await["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["entity_id"].as_str() == Some(entity_id))
        .and_then(|r| r["master_key"].as_str().map(str::to_string))
}

#[tokio::test]
async fn protect_pins_master_and_retract_restores_policy() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping protect e2e (set ER_TEST_PG_DSN to run)");
        return;
    };
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping protect e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("er_protect_{}", std::process::id()));
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
    assert_eq!(
        app.clone()
            .oneshot(op("POST", "/v1/orgs", Body::from(create.to_string())))
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );

    // Find a multi-member entity with a merge plan, and pick a member that is NOT
    // the policy master to protect — so pinning it is an observable change.
    let ids = golden_entity_ids(&app).await;
    let mut target = None;
    for id in &ids {
        let m = members(&app, id).await;
        if m.len() < 2 {
            continue;
        }
        let Some(policy_master) = plan_master(&app, id).await else {
            continue;
        };
        if let Some(victim) = m.iter().find(|k| **k != policy_master) {
            target = Some((id.clone(), policy_master, victim.clone()));
            break;
        }
    }
    let (entity_id, policy_master, victim) =
        target.expect("base_10 has a multi-member entity with a non-master member");
    assert_ne!(
        victim, policy_master,
        "victim is distinct from policy master"
    );

    // Protect the non-master record, then drain + correct (writes the overlay).
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/records/protect",
            Body::from(serde_json::json!({ "record_key": victim }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(
        pg::pending_count(&client, &oid(&client, "acme").await)
            .await
            .unwrap(),
        1
    );
    dispatcher::drain_and_correct(
        &client,
        &oid(&client, "acme").await,
        &env,
        "01PROTECTCORRECT0000000000001",
    )
    .await
    .unwrap();

    assert_eq!(
        plan_master(&app, &entity_id).await.as_deref(),
        Some(victim.as_str()),
        "protected record {victim} must be the elected master for entity {entity_id}"
    );

    // Retract the protection, drain + correct → the policy master returns.
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/records/unprotect",
            Body::from(serde_json::json!({ "record_key": victim }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    dispatcher::drain_and_correct(
        &client,
        &oid(&client, "acme").await,
        &env,
        "01PROTECTCORRECT0000000000002",
    )
    .await
    .unwrap();
    assert_eq!(
        plan_master(&app, &entity_id).await.as_deref(),
        Some(policy_master.as_str()),
        "retracting the protection restores the policy master for entity {entity_id}"
    );

    // The protection is audited.
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
            .any(|r| r["action"] == "steward.protect_record"),
        "protect_record is on the audit ledger"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
