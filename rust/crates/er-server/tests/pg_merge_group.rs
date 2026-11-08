//! The review merge dialog's bundle (spec 2.8) end to end through the
//! production path: `POST /merges` stages the assertion star + the master's
//! protect + the record-keyed field pins, the dispatcher's apply_corrections
//! drains them, and the merged entity's golden record carries exactly the
//! pinned values — the "lands exactly as shown" criterion at the lake level.
//! Gated on BOTH `ER_TEST_PG_DSN` and `DEDUPE_E2E_DUCKLAKE=1`.

use std::path::PathBuf;

use arrow::array::{Array, StringArray};
use arrow::record_batch::RecordBatch;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

use crate::common;
use crate::executor_harness;

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
        ignore_words: &[],
        deliveries: &deliveries,
        model: &model,
        run_id: "01MERGEGRPRUN000000000000001",
        ingested_at: "2026-01-01 00:00:00",
        scored_at: "2026-01-02 00:00:00",
        run_at: "2026-01-03 00:00:00",
    };
    er_cli::run_all(&session, &config, &inputs).unwrap();
}

/// Read the golden-records list as Arrow IPC batches (the data lane is binary
/// Arrow, not JSON).
async fn golden_rows(app: &axum::Router) -> Vec<RecordBatch> {
    let resp = app
        .clone()
        .oneshot(op(
            "GET",
            "/v1/orgs/acme/datasets/default/golden-records",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    arrow::ipc::reader::StreamReader::try_new(std::io::Cursor::new(bytes.to_vec()), None)
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// A Utf8 column of a batch, by name.
fn str_col<'a>(batch: &'a RecordBatch, name: &str) -> &'a StringArray {
    let idx = batch.schema().index_of(name).unwrap();
    batch
        .column(idx)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap()
}

/// The golden `(email, addr_city)` for an entity across the Arrow page.
async fn golden_fields(
    app: &axum::Router,
    entity_id: &str,
) -> Option<(Option<String>, Option<String>)> {
    for batch in &golden_rows(app).await {
        let ids = str_col(batch, "id");
        let emails = str_col(batch, "email");
        let cities = str_col(batch, "addr_city");
        for i in 0..batch.num_rows() {
            if ids.value(i) == entity_id {
                return Some((
                    emails.is_valid(i).then(|| emails.value(i).to_string()),
                    cities.is_valid(i).then(|| cities.value(i).to_string()),
                ));
            }
        }
    }
    None
}

/// The member record_keys of one entity (empty if absent → 404).
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

/// base_10's personas are one entity each (P1 = crm:C001 + billing:B001 +
/// webforms:W001, and so on), so merging crm:C001, billing:B002 and
/// webforms:W004 crosses THREE entities — the review dialog's 3-record group
/// shape, where the pair alone would have hidden the third.
#[tokio::test]
async fn merge_group_stages_star_protect_and_record_keyed_pins() {
    let Some(db) = common::fresh_db("merge_group").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping merge-group e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("er_mergegrp_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();
    let config_path = tmp.join("acme.yaml").to_string_lossy().into_owned();
    std::fs::write(&config_path, TEST_YAML).unwrap();
    // A COPY of the TF fixture, never the fixture itself: a snapshot APPENDS to
    // that file, so pointing a tenant at a committed one has the test mutate
    // the repo.
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
    let resp = app
        .clone()
        .oneshot(op("POST", "/v1/orgs", Body::from(create.to_string())))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // The merge bundle: master crm:C001, one copied field, one cleared field.
    let body = serde_json::json!({
        "records": ["crm:C001", "billing:B002", "webforms:W004"],
        "master": "crm:C001",
        "overrides": [
            { "attribute": "email", "value": "chosen@merge.test" },
            { "attribute": "addr_city", "clear": true }
        ]
    });
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/datasets/default/merges",
            Body::from(body.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let staged = json_body(resp).await;
    assert_eq!(staged["status"], "staged");
    // 2 assertions (star to master) + 1 protect + 2 overrides = 5.
    assert_eq!(staged["action_ids"].as_array().unwrap().len(), 5);

    let org_id = oid(&client, "acme").await;
    let ds = common::default_dataset(&client, "acme").await;
    let mut harness = executor_harness::Harness::start(&dsn, tmp.clone()).await;
    harness.apply_corrections(&client, &org_id, &ds).await;

    // One entity now holds the three personas' nine records…
    let mut merged_with = None;
    for batch in &golden_rows(&app).await {
        let ids = str_col(batch, "id");
        for i in 0..batch.num_rows() {
            let entity_id = ids.value(i).to_string();
            let ms = members(&app, &entity_id).await;
            if ms.contains(&"crm:C001".to_string()) {
                merged_with = Some((entity_id, ms));
                break;
            }
        }
    }
    let (entity_id, ms) = merged_with.expect("crm:C001 has an entity");
    for key in ["crm:C001", "billing:B002", "webforms:W004"] {
        assert!(ms.contains(&key.to_string()), "{key} is a member: {ms:?}");
    }
    // P1 carries a fourth record in this corpus (crm:C009), so the combined
    // entity is 4+3+3 = 10 members rather than a flat 3×3.
    assert_eq!(
        ms.len(),
        10,
        "the three personas' entities combined: {ms:?}"
    );

    // …and its golden record carries EXACTLY what the dialog pinned: the copied
    // email, the cleared city — on the entity reconcile minted, found through
    // the fresh membership, not through any id the client could have known.
    let (email, city) = golden_fields(&app, &entity_id).await.expect("golden row");
    assert_eq!(email.as_deref(), Some("chosen@merge.test"));
    assert_eq!(city, None, "the cleared field is null");

    // The master pin landed as an active protect overlay on crm:C001. This is
    // asserted at the lake rather than through merge-plans' election because
    // the merge-plans read path cannot see THIS entity: its `affected` CTE
    // keys on the entity_id an event names, an assertion merge records
    // `merged` only on the ABSORBED ids (the survivor is reachable solely
    // through `merged_into`, which nothing follows), and with every record
    // already clustered the survivor earns no `member_added` — a pre-existing
    // read-path gap (#838), not this bundle's doing. The overlay → election half is
    // covered by pg_protect_master's organic merge (a bridging record earns
    // the survivor its `member_added`).
    let session = er_cli::open_lake(
        &tmp.join("catalog.ducklake").to_string_lossy(),
        &tmp.join("data").to_string_lossy(),
    )
    .unwrap();
    let protected: Vec<String> = er_entities::record_overlay::read::active_keys_by_kind_live(
        session.connection(),
        "protect",
    )
    .unwrap()
    .into_iter()
    .map(|r| r.record_key)
    .collect();
    drop(session);
    assert_eq!(protected, vec!["crm:C001".to_string()]);

    // Validation: a master outside the group is refused…
    let bad = serde_json::json!({
        "records": ["crm:C005", "billing:B005"],
        "master": "crm:C009",
        "overrides": []
    });
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/datasets/default/merges",
            Body::from(bad.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // …and so is an attribute outside the golden set.
    let bad = serde_json::json!({
        "records": ["crm:C005", "billing:B005"],
        "master": "crm:C005",
        "overrides": [{ "attribute": "not_a_field", "value": "x" }]
    });
    let resp = app
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/datasets/default/merges",
            Body::from(bad.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // The bundle is on the audit ledger.
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
            .any(|r| r["action"] == "steward.merge_group"),
        "merge_group is on the audit ledger"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
