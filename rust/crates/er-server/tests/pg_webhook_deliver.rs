//! Webhook DELIVERY end to end (gated on `ER_TEST_PG_DSN`): register a webhook
//! pointing at a local test server, dispatch a job, and assert the subscriber
//! received exactly one POST carrying the `job.succeeded` event with a valid
//! HMAC-SHA256 signature — and that a subscriber to a different event does NOT
//! fire. No DuckLake needed (the runner is a no-op; we're testing the emit
//! path, not the pipeline). Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_webhook_deliver

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use er_server::{dispatch_pg, pg, webhook};

/// Captured deliveries: (signature header, raw body).
type Captured = Arc<Mutex<Vec<(Option<String>, Vec<u8>)>>>;

async fn record(State(cap): State<Captured>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let sig = headers
        .get(webhook::SIGNATURE_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    cap.lock().unwrap().push((sig, body.to_vec()));
    StatusCode::OK
}

#[tokio::test]
async fn webhook_fires_on_job_success_with_valid_signature() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping webhook delivery e2e (set ER_TEST_PG_DSN to run)");
        return;
    };

    // A local server recording every delivery it receives on /hook.
    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let srv = Router::new()
        .route("/hook", post(record))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, srv).await.unwrap();
    });
    let hook_url = format!("http://{addr}/hook");
    let other_url = format!("http://{addr}/hook-other");

    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS staged_steward_actions, audit_log, webhooks, schedules, \
             config_versions, api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::ensure_org(&client, "acme", "/configs/acme.yaml")
        .await
        .unwrap();

    let secret = "s3cr3t-signing-key";
    // Subscribed to job.succeeded -> must fire.
    pg::create_webhook(
        &client,
        "acme",
        &hook_url,
        &serde_json::json!(["job.succeeded"]),
        secret,
    )
    .await
    .unwrap();
    // Subscribed only to job.failed -> must NOT fire for a success.
    pg::create_webhook(
        &client,
        "acme",
        &other_url,
        &serde_json::json!(["job.failed"]),
        "other-secret",
    )
    .await
    .unwrap();

    // Enqueue + dispatch with webhook emission enabled; the runner just succeeds.
    let http = reqwest::Client::new();
    pg::enqueue(&client, "acme", "run_all_full", 0, None, None)
        .await
        .unwrap();
    let done = dispatch_pg(&client, |_job| Ok(()), Some(&http))
        .await
        .unwrap()
        .expect("a job to dispatch");
    assert_eq!(done.state, "succeeded");

    // Exactly one delivery (the job.succeeded subscriber); dispatch_pg awaited
    // the POST, so the capture is already recorded.
    let got = captured.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "only the job.succeeded subscriber fires");
    let (sig, body) = &got[0];

    // The signature authenticates the exact bytes delivered.
    assert_eq!(
        sig.as_deref(),
        Some(webhook::sign(secret, body).as_str()),
        "delivered signature verifies against the subscriber's secret"
    );

    // The body carries the event + the settled job.
    let doc: serde_json::Value = serde_json::from_slice(body).unwrap();
    assert_eq!(doc["event"], "job.succeeded");
    assert_eq!(doc["org"], "acme");
    assert_eq!(doc["state"], "succeeded");
    assert_eq!(doc["kind"], "run_all_full");
    assert!(doc["job_id"].as_str().is_some());
}
