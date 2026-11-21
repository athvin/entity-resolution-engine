//! Unmatch (spec 2.9) over Postgres + a real DuckLake, gated on BOTH
//! `ER_TEST_PG_DSN` and `DEDUPE_E2E_DUCKLAKE=1`: unmatch a member of a resolved
//! group → a `never` rule per co-member is staged → drain + correct splits the
//! record out — and because the assertions PERSIST, a later full re-analysis
//! still will not regroup it (that durability is what separates unmatch from a
//! bare split). Plus the three refusals: a singleton (422), a protected record
//! (409), and an always-closure crossing (409).
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='...' DEDUPE_E2E_DUCKLAKE=1 \
//!     cargo test -p er-server --test main pg_unmatch

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
        run_id: "01UNMATCHRUN000000000000000",
        ingested_at: "2026-01-01 00:00:00",
        scored_at: "2026-01-02 00:00:00",
        run_at: "2026-01-03 00:00:00",
    };
    er_cli::run_all(&session, &config, &inputs).unwrap();
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
            &format!("/v1/orgs/{org}/datasets/default/duplicates"),
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

async fn unmatch_splits_and_holds_across_reanalysis() {
    let Some(db) = common::fresh_db("unmatch").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping unmatch e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("er_unmatch_{}", std::process::id()));
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

    let members = first_duplicate_group(&app, "acme").await;
    let record = members[0].clone();
    let co_members: Vec<String> = members[1..].to_vec();
    let entity_id = entity_of(&er_cli::open_lake(&catalog, &data_path).unwrap(), &record)
        .expect("record is grouped");

    // The record's entity is the one the response must name.
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/datasets/default/records/unmatch",
            Body::from(serde_json::json!({ "record_key": record }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["status"], "applied");
    assert_eq!(body["entity_id"].as_str().unwrap(), entity_id);
    assert_eq!(
        body["pairs"].as_u64().unwrap() as usize,
        co_members.len(),
        "one never-rule per co-member"
    );
    assert!(body["apply_job"].as_str().is_some());

    // Every co-member got its own canonical never-rule with the unmatch note.
    let pending = pg::pending_actions(&client, &common::default_dataset(&client, "acme").await)
        .await
        .unwrap();
    assert_eq!(pending.len(), co_members.len());
    for staged in &pending {
        assert_eq!(staged.action["type"], "add_assertion");
        assert_eq!(staged.action["kind"], "never");
        assert_eq!(staged.action["note"], format!("unmatch:{entity_id}"));
        let a = staged.action["a"].as_str().unwrap();
        let b = staged.action["b"].as_str().unwrap();
        assert!(
            a == record || b == record,
            "every never-rule names the unmatched record"
        );
    }

    // Drain + correct: the record leaves the group.
    let org_id = oid(&client, "acme").await;
    let ds = common::default_dataset(&client, "acme").await;
    let mut harness = executor_harness::Harness::start(&dsn, tmp.clone()).await;
    harness.apply_corrections(&client, &org_id, &ds).await;
    {
        let session = er_cli::open_lake(&catalog, &data_path).unwrap();
        let record_entity = entity_of(&session, &record).expect("unmatched record still placed");
        for other in &co_members {
            let other_entity = entity_of(&session, other).expect("co-member remains placed");
            assert_ne!(
                record_entity, other_entity,
                "unmatch separates every current co-member"
            );
        }
    }

    // A FULL re-analysis (the next pipeline run, not a correction pass) must
    // still not regroup the record: the never-rules are ledger rows, not a
    // one-off cut. The same deliveries re-ingest idempotently.
    {
        let config = er_config::load_config_str(TEST_YAML).unwrap();
        let session = er_cli::open_lake(&catalog, &data_path).unwrap();
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
                    std::fs::read_to_string(repo_fixture(&format!("base_10/base/{s}.csv")))
                        .unwrap(),
                )
            })
            .collect();
        let model_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
        let model = er_cli::MatchModel {
            settings_json: &model_json,
            tf_csv_path: std::path::Path::new(&tf_path),
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
            run_id: "01UNMATCHRERUN00000000000",
            ingested_at: "2026-01-04 00:00:00",
            scored_at: "2026-01-05 00:00:00",
            run_at: "2026-01-06 00:00:00",
        };
        er_cli::run_all(&session, &config, &inputs).unwrap();
        let record_entity = entity_of(&session, &record).expect("unmatched record still placed");
        for other in &co_members {
            let other_entity = entity_of(&session, other).expect("co-member remains placed");
            assert_ne!(
                record_entity, other_entity,
                "full reanalysis must respect every unmatch never-rule"
            );
        }
    }

    // The unmatch is audited once per staged never-rule.
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
            .any(|r| r["action"] == "steward.add_assertion"),
        "the unmatch's never-rules are on the audit ledger"
    );
}

async fn unmatch_refuses_singletons_protected_records_and_contradictions() {
    let Some(db) = common::fresh_db("unmatch_refuse").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping unmatch refusals e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("er_unmatch_refuse_{}", std::process::id()));
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

    // A record that resolved into no group is a 422 — there is nothing to
    // unmatch it from (a pair-level never is POST /assertions).
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/datasets/default/records/unmatch",
            Body::from(serde_json::json!({ "record_key": "crm:no-such-record" }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let members = first_duplicate_group(&app, "acme").await;
    let record = members[0].clone();
    let other = members[1].clone();

    // Protect the record, then try to unmatch it → 409 (the never-cut fixpoint
    // may not sever a protected membership, so staging would only escalate).
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

    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/datasets/default/records/unmatch",
            Body::from(serde_json::json!({ "record_key": record }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "a protected record cannot be unmatched"
    );

    // Unprotect, then force the pair together with an always-rule: the unmatch
    // is then a contradiction, also a 409.
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/datasets/default/records/unprotect",
            Body::from(serde_json::json!({ "record_key": record }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/datasets/default/assertions",
            Body::from(
                serde_json::json!({ "kind": "always", "a": record, "b": other }).to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    harness.apply_corrections(&client, &org_id, &ds).await;

    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/datasets/default/records/unmatch",
            Body::from(serde_json::json!({ "record_key": record }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "an always-closure crossing refuses the unmatch"
    );
}

#[tokio::test]
async fn scenarios() {
    unmatch_splits_and_holds_across_reanalysis().await;
    unmatch_refuses_singletons_protected_records_and_contradictions().await;
}
