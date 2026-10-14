//! Unmerge over Postgres + a real DuckLake (gated on BOTH `ER_TEST_PG_DSN` and
//! `DEDUPE_E2E_DUCKLAKE=1`): peel a member off a resolved entity through the HTTP
//! surface → stage a `never` rule → drain it into the lake → re-reconcile and
//! confirm the member actually split off. Plus the S4.4.1 contradiction precheck:
//! an `always` rule that forces the pair together refuses the unmerge with 409.
//! Run with e.g.
//!   ER_TEST_PG_DSN='...' DEDUPE_E2E_DUCKLAKE=1 \
//!     cargo test -p er-server --test pg_unmerge
//!
//! The two scenarios share one DB, so they live in a SINGLE test (distinct orgs +
//! lakes) rather than racing the global schema reset across parallel threads.

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
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

/// Build a fresh per-org lake under a temp dir and run base_10 through it.
fn populate_lake(tag: &str) -> (PathBuf, String, String) {
    let tmp = std::env::temp_dir().join(format!("er_unmerge_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();

    let session = er_cli::open_lake(&catalog, &data_path).unwrap();
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
        run_id: "01UNMERGERUN0000000000000000",
        ingested_at: "2026-01-01 00:00:00",
        scored_at: "2026-01-02 00:00:00",
        run_at: "2026-01-03 00:00:00",
    };
    er_cli::run_all(&session, &config, &inputs).unwrap();
    (tmp, catalog, data_path)
}

async fn create_org(app: &axum::Router, name: &str, catalog: &str, data_path: &str) {
    let create = serde_json::json!({
        "name": name,
        "config_path": format!("/configs/{name}.yaml"),
        "env": { "ER_LAKE_CATALOG": catalog, "ER_LAKE_DATA_PATH": data_path },
    });
    let resp = app
        .clone()
        .oneshot(op("POST", "/v1/orgs", Body::from(create.to_string())))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

/// entity_id of a record_key in the current membership.
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

/// The members (record_keys) of the first duplicate group with >= 2 members.
async fn first_duplicate_group(app: &axum::Router, org: &str) -> Vec<String> {
    let resp = app
        .clone()
        .oneshot(op(
            "GET",
            &format!("/v1/orgs/{org}/duplicates"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let group = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["members"].as_array().is_some_and(|m| m.len() >= 2))
        .expect("a duplicate group with >= 2 members");
    group["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn unmerge_splits_and_respects_contradictions() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping unmerge e2e (set ER_TEST_PG_DSN to run)");
        return;
    };
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping unmerge e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

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
    let config = er_config::load_config_str(TEST_YAML).unwrap();

    // Two independent orgs so the scenarios don't perturb each other's lake.
    let (tmp_a, cat_a, data_a) = populate_lake("split");
    let (tmp_b, cat_b, data_b) = populate_lake("conflict");
    create_org(&app, "split", &cat_a, &data_a).await;
    create_org(&app, "conflict", &cat_b, &data_b).await;

    // --- Scenario A: peel a member and confirm the reconcile splits it off. ---
    let members = first_duplicate_group(&app, "split").await;
    let representative = members[0].clone();
    let peeled = members.last().unwrap().clone();
    let entity_id = entity_of(
        &er_cli::open_lake(&cat_a, &data_a).unwrap(),
        &representative,
    )
    .unwrap();

    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            &format!("/v1/orgs/split/golden-records/{entity_id}/unmerge"),
            Body::from(serde_json::json!({ "records": [peeled] }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["status"], "applied");
    assert_eq!(body["pairs"], 1);
    assert!(body["apply_job"].as_str().is_some());

    let pending = pg::pending_actions(&client, "split").await.unwrap();
    assert_eq!(pending.len(), 1, "one never-rule staged");
    {
        let session = er_cli::open_lake(&cat_a, &data_a).unwrap();
        for (action_id, action) in &pending {
            er_cli::apply_staged_action(&session, action_id, action, "2026-02-01 00:00:00")
                .unwrap();
            pg::mark_action_applied(&client, action_id).await.unwrap();
        }
        er_cli::run_reconcile(
            &session,
            &config,
            "01UNMERGERECON00000000000000",
            "2026-02-02 00:00:00",
        )
        .unwrap();
    }
    {
        let session = er_cli::open_lake(&cat_a, &data_a).unwrap();
        let rep_entity = entity_of(&session, &representative).expect("rep still placed");
        let peeled_entity = entity_of(&session, &peeled).expect("peeled still placed");
        assert_ne!(
            rep_entity, peeled_entity,
            "the never-rule the unmerge wrote separated {peeled} from {representative}"
        );
    }

    // --- Scenario B: an always-rule forbids the split (contradiction → 409). ---
    let members = first_duplicate_group(&app, "conflict").await;
    let keep = members[0].clone();
    let peel = members[1].clone();
    let entity_id = entity_of(&er_cli::open_lake(&cat_b, &data_b).unwrap(), &keep).unwrap();

    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/conflict/assertions",
            Body::from(serde_json::json!({ "kind": "always", "a": keep, "b": peel }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let pending = pg::pending_actions(&client, "conflict").await.unwrap();
    {
        let session = er_cli::open_lake(&cat_b, &data_b).unwrap();
        for (action_id, action) in &pending {
            er_cli::apply_staged_action(&session, action_id, action, "2026-02-01 00:00:00")
                .unwrap();
            pg::mark_action_applied(&client, action_id).await.unwrap();
        }
    }

    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            &format!("/v1/orgs/conflict/golden-records/{entity_id}/unmerge"),
            Body::from(serde_json::json!({ "records": [peel] }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    // The refused unmerge staged nothing new.
    assert_eq!(pg::pending_count(&client, "conflict").await.unwrap(), 0);

    let _ = std::fs::remove_dir_all(&tmp_a);
    let _ = std::fs::remove_dir_all(&tmp_b);
}
