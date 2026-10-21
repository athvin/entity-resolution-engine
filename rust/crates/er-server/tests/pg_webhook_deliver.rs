//! Webhook DELIVERY end to end (gated on `ER_TEST_PG_DSN`): register a webhook
//! pointing at a local test server and assert the subscriber received exactly
//! one POST carrying the `job.succeeded` event with a valid HMAC-SHA256
//! signature — and that a subscriber to a different event does NOT fire. No
//! DuckLake needed (we're testing the emit path, not the pipeline).
//!
//! The delivery proof goes through `webhook::emit_with_egress` with
//! `Egress::Unguarded`, because the listener it POSTs to is on `127.0.0.1` —
//! which the outbound-URL guard exists to refuse. That seam is test-only and
//! `tests/egress_guard.rs` forbids product code from naming it. The companion
//! test below runs the GUARDED path over the same local subscriber and asserts
//! the delivery is refused, so the guard is proven live rather than assumed.
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_webhook_deliver

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use er_server::urlguard::Egress;
use er_server::{dispatch_pg, pg, webhook};

/// The org's id for a test org name (child wrappers key on org_id).
async fn oid(client: &tokio_postgres::Client, name: &str) -> String {
    er_server::pg::org_id_by_name(client, name)
        .await
        .expect("org lookup")
        .expect("org exists")
}

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

/// Delivery, in two phases of ONE test.
///
/// Not two `#[tokio::test]`s: cargo runs a binary's tests on parallel threads,
/// and two tests that each `DROP TABLE ... CASCADE` the shared schema deadlock
/// against each other — one's drop takes a lock the other's insert needs, via
/// the `orgs_history` trigger. `pg_scheduler_tick.rs` says the same thing for
/// the same reason. Running locally with `--test-threads=1` hides it; CI does
/// not pass that flag, which is how this was caught.
#[tokio::test]
async fn webhook_delivery_fires_signed_and_the_guard_refuses_a_blocked_address() {
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
        &oid(&client, "acme").await,
        &hook_url,
        &serde_json::json!(["job.succeeded"]),
        secret,
    )
    .await
    .unwrap();
    // Subscribed only to job.failed -> must NOT fire for a success.
    pg::create_webhook(
        &client,
        &oid(&client, "acme").await,
        &other_url,
        &serde_json::json!(["job.failed"]),
        "other-secret",
    )
    .await
    .unwrap();

    // Emit the settled-job event directly. Unguarded, because the subscriber is
    // a loopback listener; see the module note.
    let http = reqwest::Client::new();
    let org_id = oid(&client, "acme").await;
    let results = webhook::emit_with_egress(
        &client,
        &http,
        &org_id,
        "acme",
        "job.succeeded",
        &serde_json::json!({
            "job_id": "01JOBDELIVER00000000000000",
            "kind": "run_all_full",
            "state": "succeeded",
        }),
        Egress::Unguarded,
    )
    .await
    .unwrap();
    assert_eq!(results.len(), 1, "only the job.succeeded subscriber fires");
    assert_eq!(
        results[0].status,
        Some(200),
        "the subscriber accepted the delivery: {:?}",
        results[0].error
    );

    // Exactly one delivery (the job.succeeded subscriber); emit awaited the
    // POST, so the capture is already recorded.
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

    // ---------------------------------------------------------------- phase 2
    // The same loopback subscriber, reached through the GUARDED path — so the
    // guard is proven live on the real dispatch flow rather than assumed from
    // the fact that it is implemented.
    //
    // `https` deliberately: with `http` the guard refuses on the scheme and
    // this would prove only that, never reaching the address check that stops
    // `169.254.169.254`. The listener speaks plain HTTP, which does not matter
    // — the delivery is refused before a connection is attempted.
    captured.lock().unwrap().clear();
    client
        .execute("DELETE FROM webhooks", &[])
        .await
        .expect("clear the phase-1 subscribers");
    pg::create_webhook(
        &client,
        &org_id,
        &format!("https://{addr}/hook"),
        &serde_json::json!(["job.succeeded"]),
        secret,
    )
    .await
    .unwrap();

    pg::enqueue(&client, &org_id, "run_all_full", 0, None, None)
        .await
        .unwrap();
    let done = dispatch_pg(&client, |_job| Ok(()), Some(&http))
        .await
        .unwrap()
        .expect("a job to dispatch");
    assert_eq!(
        done.state, "succeeded",
        "a webhook the server may not reach must not fail the job"
    );
    assert!(
        captured.lock().unwrap().is_empty(),
        "the guarded path must not have POSTed to a loopback subscriber"
    );

    // And the refusal is legible, naming why rather than looking like a timeout.
    // This is the lock for the `169.254.169.254` case the repo's security notes
    // call the headline SSRF. It proves the APPLICATION layer only: whether a
    // pod can reach instance metadata at all is a NetworkPolicy question
    // answered by the manual `docs/infrastructure.md` §17 probe.
    let refused = webhook::emit_job_event(&client, &http, &done)
        .await
        .unwrap();
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].status, None, "nothing was delivered");
    let error = refused[0].error.clone().unwrap_or_default();
    assert!(
        error.contains("refused") && error.contains("loopback"),
        "the recorded error should say what was refused and why: {error}"
    );
    assert!(
        captured.lock().unwrap().is_empty(),
        "still nothing delivered"
    );
}
