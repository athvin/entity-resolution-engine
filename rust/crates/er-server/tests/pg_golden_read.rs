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
    // The org's config on disk, so the read API can resolve the auto-merge
    // threshold for the run ledger's per-run auto-merge count (PR 1.5b).
    let config_path = tmp.join("config.yaml");
    std::fs::write(&config_path, TEST_YAML).unwrap();
    let config_path = config_path.to_string_lossy().into_owned();
    populate_lake(&catalog, &data_path);

    // An org whose env points the read API at that lake.
    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS staged_steward_actions, audit_log, schedules, config_versions, api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    let create = serde_json::json!({
        "name": "acme",
        "config_path": "/configs/acme.yaml",
        "env": { "ER_LAKE_CATALOG": catalog, "ER_LAKE_DATA_PATH": data_path, "ER_CONFIG_PATH": config_path },
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
    let some_key = detail["members"][0]["record_key"]
        .as_str()
        .unwrap()
        .to_string();

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
    // run_all populates review_queue, so the gray-band pair is an open review.
    assert!(m["open_reviews"].as_i64().unwrap() >= 1);
    assert!(m["snapshot"].as_i64().unwrap() >= 1);

    // Data quality (read-only): base_10 scores over the same lake. 23 records,
    // 10 entities; every rate + the composite score in [0,1]; base_10 has dupes.
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/data-quality"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let dq = json_body(resp).await;
    assert_eq!(dq["records"], 23);
    assert_eq!(dq["entities"], 10);
    assert_eq!(dq["stale_days"], 365);
    let score = dq["dq_score"].as_f64().unwrap();
    assert!((0.0..=1.0).contains(&score), "dq_score in [0,1]");
    let comp = dq["completeness"]["overall"].as_f64().unwrap();
    assert!((0.0..=1.0).contains(&comp), "completeness in [0,1]");
    assert!(
        dq["completeness"]["by_attribute"]["given_name"].is_number(),
        "per-attribute completeness present"
    );
    let dup = dq["duplicate_rate"].as_f64().unwrap();
    assert!(
        dup > 0.0 && dup <= 1.0,
        "base_10 has a nonzero duplicate rate"
    );
    assert!(dq["junk_rate"].is_number() && dq["stale_rate"].is_number());
    // The stale_days query param is honoured (presentation parameter, not config).
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/data-quality?stale_days=1"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["stale_days"], 1);
    // READ-ONLY: data-quality committed nothing — the metrics snapshot is unchanged.
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/metrics"))
        .await
        .unwrap();
    assert_eq!(
        json_body(resp).await["snapshot"],
        m["snapshot"],
        "data-quality must not commit a new snapshot"
    );

    // Duplicates: groups with >=2 members, each carrying its member list.
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/duplicates"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let dups = json_body(resp).await;
    let groups = dups["items"].as_array().unwrap();
    assert!(!groups.is_empty(), "base_10 has duplicate groups");
    assert!(groups[0]["member_count"].as_i64().unwrap() >= 2);
    assert!(groups[0]["members"].as_array().unwrap().len() >= 2);

    // Match-scores: base_10 has 19 active scored pairs with evidence.
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/match-scores"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let scores = json_body(resp).await;
    let pairs = scores["items"].as_array().unwrap();
    assert_eq!(pairs.len(), 19, "base_10 scores 19 active pairs");
    assert!(pairs[0].get("rec_a_key").is_some());
    assert!(pairs[0].get("match_probability").is_some());
    assert!(pairs[0].get("evidence").is_some());

    // Probability band: min=0.95 keeps only the high-confidence pairs.
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/match-scores?min=0.95"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let banded = json_body(resp).await;
    let hi = banded["items"].as_array().unwrap();
    assert!(hi.len() <= pairs.len(), "band narrows the set");
    assert!(
        hi.iter()
            .all(|p| p["match_probability"].as_f64().unwrap() >= 0.95),
        "every banded pair is >= 0.95"
    );

    // Records-by-key: look up a real standardized record by its record_key.
    let resp = app
        .clone()
        .oneshot(op_get(&format!("/v1/orgs/acme/records?key={some_key}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let recs = json_body(resp).await;
    let got = recs["items"].as_array().unwrap();
    assert_eq!(got.len(), 1, "one record for the looked-up key");
    assert_eq!(got[0]["record_key"], some_key);
    assert!(got[0].get("source_system").is_some());
    assert!(got[0]["attributes"].is_object(), "attributes nested");
    assert!(recs["snapshot"].as_i64().unwrap() >= 1);

    // No key params => empty list (not an error).
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/records"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["items"].as_array().unwrap().len(), 0);

    // Reviews: run_all populated review_queue with base_10's gray-band pair(s)
    // (probability in [review_low, auto_merge)). The shared-household trap at
    // ~0.857 is one such open gray_band review.
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/reviews"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let reviews = json_body(resp).await;
    let review_items = reviews["items"].as_array().unwrap();
    let non_empty = !review_items.is_empty();
    assert!(non_empty, "base_10 has a gray-band review");
    assert_eq!(review_items[0]["reason"], "gray_band");
    assert_eq!(review_items[0]["status"], "open");
    assert_eq!(review_items[0]["subject_type"], "pair");
    let review_id = review_items[0]["review_id"].as_str().unwrap().to_string();

    // review-by-id resolves that review.
    let resp = app
        .clone()
        .oneshot(op_get(&format!("/v1/orgs/acme/reviews/{review_id}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["review_id"], review_id);

    // assertions are still empty (no steward writes in a base run).
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/assertions"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["items"].as_array().unwrap().len(), 0);

    // imports: run_all wrote one ingest_batches receipt per source (crm/billing/
    // webforms), each all-new for the base delivery.
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/imports"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let imports = json_body(resp).await;
    let rows = imports["items"].as_array().unwrap();
    assert_eq!(rows.len(), 3, "one receipt per delivered source");
    let total_new: i64 = rows.iter().map(|r| r["new_count"].as_i64().unwrap()).sum();
    assert_eq!(total_new, 23, "base_10 lands 23 new records");

    // Runs: run_all wrote a `runs` ledger row (full mode, succeeded).
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/runs"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let runs = json_body(resp).await;
    let run_arr = runs.as_array().unwrap();
    assert_eq!(run_arr.len(), 1, "run_all recorded one run");
    assert_eq!(run_arr[0]["mode"], "full");
    assert_eq!(run_arr[0]["status"], "succeeded");
    assert!(run_arr[0]["config_hash"].as_str().is_some());
    // PR 1.5b: the per-run auto-merge count. base_10 resolves 23 records to 10
    // entities purely by score (no assertions), so the full run scored at least
    // one pair at or above the auto-merge threshold and it attributes to this run.
    let auto_merged = run_arr[0]["auto_merged_pairs"]
        .as_u64()
        .expect("runs row carries a numeric auto_merged_pairs");
    assert!(
        auto_merged >= 1,
        "the full run auto-merged at least one scored pair, got {auto_merged}"
    );

    // A review by id is a 404 when there's no review_queue / no such review.
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/reviews/no-such-review"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Merge-plans (default most_attributes policy): the affected→masters→join
    // query executes. base_10's base resolution emits only `created` events (no
    // merged/member_added), so `affected` — and the plan — is correctly empty;
    // this proves the master-election SQL is valid and runs. (A merge-scenario
    // lake yields rows.)
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/merge-plans"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(json_body(resp).await["items"].is_array());

    // The other config-free policies each execute their own ORDER BY.
    for policy in ["most_recent", "most_complete", "oldest"] {
        let resp = app
            .clone()
            .oneshot(op_get(&format!(
                "/v1/orgs/acme/merge-plans?policy={policy}"
            )))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "policy {policy}");
    }

    // An unknown policy is a 422.
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/merge-plans?policy=nonsense"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // CSV format: a text/csv body with the header row.
    let resp = app
        .clone()
        .oneshot(op_get("/v1/orgs/acme/merge-plans?format=csv"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/csv")
    );
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let csv = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(csv.starts_with("entity_id,master_key,member_count,member_records,"));
    assert!(csv.lines().count() >= 1, "at least the header row");

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
