//! Nickname-lexicon editing end to end (gated on `ER_TEST_PG_DSN`): create an
//! org with an `ER_LEXICON_PATH`, PUT a lexicon (proving canonicalisation +
//! validation), GET it back, and confirm an apply (`run_all_full`) job was
//! enqueued. The endpoints touch only Postgres + the per-org CSV file — no lake
//! — so this needs Postgres but not DuckLake. Own test binary (tests in one
//! binary run in parallel and would race the shared schema with other pg_* tests).

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

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

#[tokio::test]
async fn lexicon_round_trip_canonicalises_and_enqueues_rebuild() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping lexicon e2e (set ER_TEST_PG_DSN to run)");
        return;
    };

    let tmp = std::env::temp_dir().join(format!("er_lexicon_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let lex_path = tmp.join("lexicon.csv").to_string_lossy().into_owned();

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

    let create = serde_json::json!({
        "name": "acme",
        "config_path": "/configs/acme.yaml",
        "env": { "ER_LEXICON_PATH": lex_path },
    });
    let resp = app
        .clone()
        .oneshot(op("POST", "/v1/orgs", Body::from(create.to_string())))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // PUT a lexicon with a non-canonical pair ("tony","Anthony" → "anthony","tony").
    let put = serde_json::json!({
        "pairs": [
            { "variant_a": "Bob", "variant_b": "Robert" },
            { "variant_a": "tony", "variant_b": "Anthony" }
        ]
    });
    let resp = app
        .clone()
        .oneshot(op(
            "PUT",
            "/v1/orgs/acme/lexicon",
            Body::from(put.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert!(
        body["apply_job"].as_str().is_some(),
        "PUT enqueues a run_all_full apply job"
    );
    let pairs = body["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 2);
    // Canonicalised + sorted (min<max within a pair, set-ordered across pairs).
    assert_eq!(pairs[0]["variant_a"], "anthony");
    assert_eq!(pairs[0]["variant_b"], "tony");
    assert_eq!(pairs[1]["variant_a"], "bob");
    assert_eq!(pairs[1]["variant_b"], "robert");

    // The CSV was written (header + 2 rows).
    let csv = std::fs::read_to_string(&lex_path).unwrap();
    assert!(csv.starts_with("variant_a,variant_b\n"));
    assert!(csv.contains("bob,robert"));
    assert!(csv.contains("anthony,tony"));

    // GET round-trips the stored pairs.
    let resp = app
        .clone()
        .oneshot(op("GET", "/v1/orgs/acme/lexicon", Body::empty()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let got = json_body(resp).await;
    assert_eq!(got["pairs"].as_array().unwrap().len(), 2);

    // Validation: a self-variant pair is rejected.
    let bad = serde_json::json!({ "pairs": [{ "variant_a": "al", "variant_b": "Al" }] });
    let resp = app
        .oneshot(op(
            "PUT",
            "/v1/orgs/acme/lexicon",
            Body::from(bad.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let _ = std::fs::remove_dir_all(&tmp);
}
