//! Ignore-words dictionary editing end to end (gated on `ER_TEST_PG_DSN`):
//! create an org with an `ER_IGNORE_WORDS_PATH`, PUT a dictionary (proving
//! canonicalisation + validation), GET it back, and confirm an apply
//! (`run_all_full`) job was enqueued. The endpoints touch only Postgres + the
//! per-org CSV file — no lake — so this needs Postgres but not DuckLake. Own
//! test binary (tests in one binary run in parallel and would race the shared
//! schema with other pg_* tests).

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

use crate::common;

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
async fn ignore_words_round_trip_canonicalises_and_enqueues_rebuild() {
    let Some(db) = common::fresh_db("ignore_words").await else {
        return;
    };
    let dsn = db.dsn().to_string();

    let tmp = std::env::temp_dir().join(format!("er_ignore_words_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let words_path = tmp.join("ignore_words.csv").to_string_lossy().into_owned();

    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    let create = serde_json::json!({
        "name": "acme",
        "config_path": "/configs/acme.yaml",
        "env": { "ER_IGNORE_WORDS_PATH": words_path },
    });
    let resp = app
        .clone()
        .oneshot(op("POST", "/v1/orgs", Body::from(create.to_string())))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // PUT rows with a non-canonical word ("PTY" → "pty") and a duplicate that
    // canonicalisation collapses.
    let put = serde_json::json!({
        "rows": [
            { "word": "PTY", "dictionary": "Legal_Suffixes" },
            { "word": "pty", "dictionary": "legal_suffixes" },
            { "word": "jr", "dictionary": "name_titles" }
        ]
    });
    let resp = app
        .clone()
        .oneshot(op(
            "PUT",
            "/v1/orgs/acme/datasets/default/ignore-words",
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
    let rows = body["rows"].as_array().unwrap();
    // Canonicalised (trimmed + lowercased), deduped, set-ordered.
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["word"], "jr");
    assert_eq!(rows[0]["dictionary"], "name_titles");
    assert_eq!(rows[1]["word"], "pty");
    assert_eq!(rows[1]["dictionary"], "legal_suffixes");

    // The CSV was written (header + 2 rows).
    let csv = std::fs::read_to_string(&words_path).unwrap();
    assert!(csv.starts_with("word,dictionary\n"));
    assert!(csv.contains("pty,legal_suffixes"));
    assert!(csv.contains("jr,name_titles"));

    // GET round-trips the stored rows.
    let resp = app
        .clone()
        .oneshot(op(
            "GET",
            "/v1/orgs/acme/datasets/default/ignore-words",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let got = json_body(resp).await;
    assert_eq!(got["rows"].as_array().unwrap().len(), 2);

    // Validation: a whitespace-bearing word can never match a
    // whitespace-split token — refused.
    let bad = serde_json::json!({ "rows": [{ "word": "de la", "dictionary": "particles" }] });
    let resp = app
        .clone()
        .oneshot(op(
            "PUT",
            "/v1/orgs/acme/datasets/default/ignore-words",
            Body::from(bad.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Validation: a comma would corrupt the CSV — refused.
    let bad = serde_json::json!({ "rows": [{ "word": "a,b", "dictionary": "x" }] });
    let resp = app
        .oneshot(op(
            "PUT",
            "/v1/orgs/acme/datasets/default/ignore-words",
            Body::from(bad.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let _ = std::fs::remove_dir_all(&tmp);
}
