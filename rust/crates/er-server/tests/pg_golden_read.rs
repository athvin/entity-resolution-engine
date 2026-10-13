//! The golden-records read API over a real per-org DuckLake (gated on BOTH
//! `ER_TEST_PG_DSN` and `DEDUPE_E2E_DUCKLAKE=1`): an org whose stored env points
//! at a lake, populated by `er_cli::run_all` over base_10, read back through
//! `GET /v1/orgs/{org}/golden-records` — list, keyset paging, contains-search,
//! and the malformed-cursor 422. Run with e.g.
//!   ER_TEST_PG_DSN='...' DEDUPE_E2E_DUCKLAKE=1 \
//!     cargo test -p er-server --test pg_golden_read

use std::collections::BTreeSet;
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

fn op_get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer op-secret")
        .body(Body::empty())
        .unwrap()
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Write base_10's resolved results into a fresh DuckLake at `(catalog, data)`.
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
        run_id: "01READRUN000000000000000000",
        ingested_at: "2026-01-01 00:00:00",
        scored_at: "2026-01-02 00:00:00",
        run_at: "2026-01-03 00:00:00",
    };
    er_cli::run_all(&session, &config, &inputs).unwrap();
    // Session dropped here: the writer releases the catalog before the read API
    // attaches it read-only.
}

#[tokio::test]
async fn golden_records_read_over_ducklake() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping golden-read e2e (set ER_TEST_PG_DSN to run)");
        return;
    };
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping golden-read e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    // A fresh lake, populated with base_10's 10 resolved entities.
    let tmp = std::env::temp_dir().join(format!("er_read_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();
    populate_lake(&catalog, &data_path);

    // An org whose env points the read API at that lake.
    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute("DROP TABLE IF EXISTS config_versions, api_keys, jobs, orgs CASCADE;")
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    let create = serde_json::json!({
        "name": "acme",
        "config_path": "/configs/acme.yaml",
        "env": { "ER_LAKE_CATALOG": catalog, "ER_LAKE_DATA_PATH": data_path },
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/orgs")
                .header(header::AUTHORIZATION, "Bearer op-secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(create.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Full list: all 10 entities, a pinned snapshot, no next page.
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/golden-records"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let page = json_body(resp).await;
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 10, "base_10 resolves to 10 golden records");
    assert!(page["snapshot"].as_i64().unwrap() >= 1);
    assert!(page["next_cursor"].is_null());
    assert!(items[0].get("entity_id").is_some());
    assert!(items[0].get("given_name").is_some());
    let some_entity = items[0]["entity_id"].as_str().unwrap().to_string();

    // Keyset paging: limit=4 walks all 10 in 3 pages with no dup/missing.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut uri = "/v1/orgs/acme/golden-records?limit=4".to_string();
    let mut pages = 0;
    loop {
        let resp = app.clone().oneshot(op_get(&uri)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let page = json_body(resp).await;
        for item in page["items"].as_array().unwrap() {
            let id = item["entity_id"].as_str().unwrap().to_string();
            assert!(seen.insert(id), "entity_id must not repeat across pages");
        }
        pages += 1;
        match page["next_cursor"].as_str() {
            Some(c) => uri = format!("/v1/orgs/acme/golden-records?limit=4&cursor={c}"),
            None => break,
        }
        assert!(pages < 10, "paging must terminate");
    }
    assert_eq!(seen.len(), 10, "paging covered every entity");
    assert_eq!(pages, 3, "10 records at limit 4 => 3 pages");

    // Contains-search: a term in no name yields an empty page.
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/golden-records?q=zzzznotarealname"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["items"].as_array().unwrap().len(), 0);

    // A malformed cursor is a 422.
    let resp = app
        .clone()
        .oneshot(op_get(
            "/v1/orgs/acme/golden-records?cursor=%%%not-base64%%%",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Detail: golden + members + lineage + events for a real entity.
    let resp = app
        .clone()
        .oneshot(op_get(&format!(
            "/v1/orgs/acme/golden-records/{some_entity}"
        )))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let detail = json_body(resp).await;
    assert_eq!(detail["golden"]["entity_id"], some_entity);
    assert!(
        !detail["members"].as_array().unwrap().is_empty(),
        "an entity has >=1 member"
    );
    assert!(detail["lineage"].is_array());
    assert!(detail["events"].is_array());

    // Detail of an absent entity is a 404.
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/golden-records/no-such-entity"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Metrics: base_10 => 10 entities, 23 current std records, some dup groups.
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/metrics"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let m = json_body(resp).await;
    assert_eq!(m["entities"], 10);
    assert_eq!(m["records"], 23);
    assert!(m["duplicate_groups"].as_i64().unwrap() >= 1);
    assert_eq!(m["open_reviews"], 0);
    assert!(m["snapshot"].as_i64().unwrap() >= 1);

    // A viewer key can read (viewer+).
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/orgs/acme/api-keys")
                .header(header::AUTHORIZATION, "Bearer op-secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"role":"viewer"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let viewer_key = json_body(resp).await["key"].as_str().unwrap().to_string();
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/orgs/acme/golden-records")
                .header(header::AUTHORIZATION, format!("Bearer {viewer_key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let _ = std::fs::remove_dir_all(&tmp);
}
