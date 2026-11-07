//! An `apply_corrections` job applies its staged actions on the TASK path.
//!
//! `pg_drain_dispatcher.rs` makes the same claim about the legacy in-process
//! arm (`dispatcher::drain_and_correct`). This makes it about the executor —
//! the only path a job takes on a cluster, since `deploy.yml` lists every kind
//! in `DAG_KINDS` — and nothing did, which is how #760 survived.
//!
//! What #760 was: `apply_staged` read the actions it applies from
//! `order.params["staged_actions"]`, a key nothing in the tree ever wrote.
//! `compose_input` passes `job.params` verbatim, so the array was always
//! absent, and an absent array was an empty pass rather than a failure. The
//! job settled **succeeded** having applied nothing, the actions stayed
//! pending, and every later pass re-drained and re-discarded them. Assertions,
//! merges, unmerges and bulk resolves were going nowhere in dev and
//! production, silently.
//!
//! The fix routes the payloads along `drain_actions`' typed edge, which is
//! what this exercises end to end: a real control plane, a real DuckLake, the
//! real `exec-task` binary in a fake pod, and the staged rows read back out of
//! Postgres afterwards.
//!
//! Deliberately NOT asserted here: the `entity.write_back` webhook. That is a
//! second gap in the same family — `write_back_resolutions` lives only in the
//! legacy arm and the task path never emits it — and it needs a pod to read
//! the corrected corpus and a control task to queue the event, which is a
//! decomposition rather than a fix. No subscriber is registered below, so the
//! emitter is not reached at all and this file makes no claim about it.
//!
//! Its own file, like `pg_drain_dispatcher.rs`, because it DROPs and recreates
//! the control-plane schema and creates org `acme`. Gated on both
//! `ER_TEST_PG_DSN` and `DEDUPE_E2E_DUCKLAKE=1`.

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, dispatcher, pg};
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

/// A resolved corpus to correct: the same `run_all` the CLI gate runs, so the
/// reviews the test resolves are real ones.
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
        run_id: "01APPLYCORRDAG0000000000000",
        ingested_at: "2026-01-01 00:00:00",
        scored_at: "2026-01-02 00:00:00",
        run_at: "2026-01-03 00:00:00",
    };
    er_cli::run_all(&session, &config, &inputs).unwrap();
}

#[tokio::test]
async fn apply_corrections_applies_its_staged_actions_through_the_executor() {
    let Some(db) = common::fresh_db("apply_corrections_dag").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping the apply_corrections dag e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("er_apply_corr_dag_{}", std::process::id()));
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
    // The full artefact set a provisioned org declares — `init` resolves the
    // run context through all of them, where the legacy arm's `run_correct`
    // needed only the config.
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
        .oneshot(op(
            "GET",
            "/v1/orgs/acme/datasets/default/reviews",
            Body::empty(),
        ))
        .await
        .unwrap();
    let review_id = json_body(resp).await["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Stage two actions through the real endpoints: an assertion and a review
    // resolution. Staging only — applying them is the job's work.
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/datasets/default/assertions",
            Body::from(r#"{"kind":"always","a":"crm:C001","b":"billing:B001"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            &format!("/v1/orgs/acme/datasets/default/reviews/{review_id}/resolve"),
            Body::from(r#"{"resolution":"match"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let ds = common::default_dataset(&client, "acme").await;
    assert_eq!(
        pg::pending_count(&client, &ds).await.unwrap(),
        2,
        "two actions are staged and waiting for a correction pass"
    );

    // The job, claimed and planned by the executor, then run as task pods.
    let org_id = pg::org_id_by_name(&client, "acme")
        .await
        .unwrap()
        .expect("org");
    let mut harness = executor_harness::Harness::start(&dsn, tmp.clone()).await;
    let queued = pg::enqueue(
        &client,
        &org_id,
        Some(&ds),
        "apply_corrections",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    assert!(
        dispatcher::dispatch_once(&client, Some(&mut harness.executor))
            .await
            .unwrap(),
        "the executor claims and plans the apply_corrections job"
    );
    let settled = harness
        .run_to_settlement(&client, &org_id, &queued.id)
        .await;
    let rows = er_entities::task_instance::read::by_job(&client, &org_id, &queued.id)
        .await
        .unwrap();
    let shape: Vec<(String, String, Option<String>)> = rows
        .iter()
        .map(|r| (r.task_id.clone(), r.state.clone(), r.error_detail.clone()))
        .collect();
    assert_eq!(
        settled, "succeeded",
        "the job settled {settled:?}: {shape:?}"
    );

    // THE assertion #760 is about. Before the fix every one of these task rows
    // read `succeeded` and this count was still 2 — which is exactly why a
    // settled job is not evidence that it did anything.
    assert_eq!(
        pg::pending_count(&client, &ds).await.unwrap(),
        0,
        "the pass must apply AND mark every staged action; task rows {shape:?}"
    );

    // Applied in the lake, not merely marked in Postgres: the assertion the
    // steward added is in force, and so is the one the match resolution
    // derived.
    let resp = app
        .clone()
        .oneshot(op(
            "GET",
            "/v1/orgs/acme/datasets/default/assertions",
            Body::empty(),
        ))
        .await
        .unwrap();
    let items = json_body(resp).await["items"].as_array().unwrap().clone();
    assert_eq!(
        items.len(),
        2,
        "add_assertion + the match-derived assertion: {items:?}"
    );
    assert!(items.iter().all(|r| r["active"] == true), "{items:?}");

    // And the resolution survived the correction pass that followed it.
    let resp = app
        .oneshot(op(
            "GET",
            &format!("/v1/orgs/acme/datasets/default/reviews/{review_id}"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["status"], "resolved_match");

    let _ = std::fs::remove_dir_all(&tmp);
}
