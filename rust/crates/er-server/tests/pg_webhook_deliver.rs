//! Webhook delivery end to end, through the outbox (gated on
//! `ER_TEST_PG_DSN`). No DuckLake needed — this is the delivery path, not the
//! pipeline.
//!
//! Three things, in three phases of one test:
//!
//! 1. **Recording is not sending.** `emit` writes the event and returns; the
//!    POST happens when the queue is drained. That split is the whole point:
//!    before it, every emit site POSTed inline and dropped the outcome with
//!    `let _ =`, so a subscriber unreachable for a moment missed the event
//!    permanently and nobody — tenant or operator — could find out.
//! 2. **The outbound-URL guard is live on the real path**, not merely
//!    implemented: the same loopback subscriber is refused when drained
//!    guarded, and the refusal is terminal rather than retried.
//! 3. **A transient failure is retried and a permanent one is not** — the
//!    behaviour the inline-and-discard path could not have had.
//!
//! The delivery proof drains with `Egress::Unguarded`, because the listener it
//! POSTs to is on `127.0.0.1`, which the guard exists to refuse. That seam is
//! test-only and `tests/egress_guard.rs` forbids product code from naming it.
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_webhook_deliver

use std::sync::atomic::{AtomicUsize, Ordering};
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

/// A listener that fails its first `fail_first` requests with 503 and then
/// succeeds — a subscriber restarting, which is the case the outbox exists for.
#[derive(Clone)]
struct Flaky {
    hits: Arc<AtomicUsize>,
    fail_first: usize,
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
}

async fn flaky(State(s): State<Flaky>, _h: HeaderMap, body: Bytes) -> StatusCode {
    let n = s.hits.fetch_add(1, Ordering::SeqCst);
    s.bodies.lock().unwrap().push(body.to_vec());
    if n < s.fail_first {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    }
}

/// A listener that always rejects with 400 — a subscriber that will never
/// accept this request, however many times it arrives.
async fn always_bad_request() -> StatusCode {
    StatusCode::BAD_REQUEST
}

/// Delivery, in three phases of ONE test.
///
/// One test, not three, for two independent reasons — and neither shows up
/// unless you run the tier the way CI does (no `--test-threads=1`):
///
/// 1. Cargo runs a binary's tests on parallel threads, and two tests that each
///    `DROP TABLE ... CASCADE` the shared schema deadlock against each other —
///    one's drop takes a lock the other's insert needs, via the `orgs_history`
///    trigger.
/// 2. The outbox's `claim` is global across orgs **by design**: one queue, one
///    drainer, oldest first. So two concurrent tests do not merely race on the
///    schema, they steal each other's deliveries. `pg_scheduler_tick.rs` was
///    pushed into the same shape by the same property of `schedule_tick`.
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
            // webhook_deliveries explicitly: `DROP TABLE webhooks CASCADE`
            // drops the dependent FK CONSTRAINT, not the referencing table, so
            // without this the outbox keeps rows pointing at webhooks that no
            // longer exist — and because `claim` is global and oldest-first,
            // the drain picks up another run's orphan instead of this test's
            // event.
            "DROP TABLE IF EXISTS webhook_deliveries, staged_steward_actions, audit_log, \
             webhooks, schedules, config_versions, api_keys, jobs, orgs CASCADE;",
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

    // Recording the event must not send it. That split is the whole point of
    // the outbox: before it, every emit site POSTed inline and dropped the
    // outcome with `let _ =`, so a subscriber unreachable for a moment missed
    // the event permanently and nobody could tell.
    let http = webhook::client();
    let org_id = oid(&client, "acme").await;
    let queued = webhook::emit(
        &client,
        &org_id,
        "acme",
        "job.succeeded",
        &serde_json::json!({
            "job_id": "01JOBDELIVER00000000000000",
            "kind": "run_all_full",
            "state": "succeeded",
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        queued.len(),
        1,
        "only the job.succeeded subscriber is queued"
    );
    assert_eq!(queued[0].state, "pending");
    assert_eq!(queued[0].attempt, 0, "nothing attempted yet");
    assert!(
        captured.lock().unwrap().is_empty(),
        "emit must RECORD, not send"
    );

    // Now drain it. Unguarded, because the subscriber is a loopback listener;
    // see the module note.
    let drained = webhook::drain_once_with_egress(&client, &http, Egress::Unguarded)
        .await
        .unwrap()
        .expect("a delivery was due");
    assert_eq!(drained.state, "delivered", "{:?}", drained.error);
    assert_eq!(drained.status, Some(200));
    // And the queue is now empty, so a second drain is a no-op rather than a
    // duplicate delivery.
    assert!(
        webhook::drain_once_with_egress(&client, &http, Egress::Unguarded)
            .await
            .unwrap()
            .is_none(),
        "a delivered event must not be re-delivered"
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
        .execute("DELETE FROM webhook_deliveries", &[])
        .await
        .expect("clear the phase-1 outbox");
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

    // The refusal is recorded against the delivery, legibly, naming why rather
    // than looking like a timeout — and it is TERMINAL rather than retried five
    // more times, because a destination the server may not reach will still be
    // one in five minutes.
    //
    // This is the lock for the `169.254.169.254` case the repo's security notes
    // call the headline SSRF. It proves the APPLICATION layer only: whether a
    // pod can reach instance metadata at all is a NetworkPolicy question
    // answered by the manual `docs/infrastructure.md` §17 probe, and nothing
    // here speaks to it.
    let guarded_log = er_entities::webhook_delivery::read::page(&client, &org_id, None, None, 50)
        .await
        .unwrap();
    assert_eq!(
        guarded_log.len(),
        1,
        "one delivery recorded for the guarded attempt"
    );
    assert_eq!(
        guarded_log[0].state, "failed",
        "a refused destination is terminal"
    );
    assert_eq!(guarded_log[0].attempt, 1, "attempted once, not retried");
    assert_eq!(
        guarded_log[0].response_code, None,
        "no response — nothing was sent"
    );
    let error = guarded_log[0].error.clone().unwrap_or_default();
    assert!(
        error.contains("refused") && error.contains("loopback"),
        "the recorded error should say what was refused and why: {error}"
    );
    assert!(
        captured.lock().unwrap().is_empty(),
        "still nothing delivered"
    );

    // ---------------------------------------------------------------- phase 3
    // The failing path, which is what the outbox is FOR. Delivery used to be a
    // `let _ = emit_job_event(...)` at three call sites: the POST happened
    // inline and its outcome was discarded, so a subscriber unreachable for one
    // second missed the event permanently and nobody could find out.
    //
    // So: a transient failure must be retried, a hopeless one must be given up
    // on rather than retried forever, and both must be legible afterwards.
    let flaky_state = Flaky {
        hits: Arc::new(AtomicUsize::new(0)),
        fail_first: 2,
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let srv = Router::new()
        .route("/flaky", post(flaky))
        .with_state(flaky_state.clone())
        .merge(Router::new().route("/nope", post(always_bad_request)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, srv).await.unwrap();
    });

    // The same org and schema as the phases above — only the subscribers and
    // the outbox are reset. Re-dropping mid-test would mint a new org id and
    // strand `org_id`, which is a thing to notice: these phases share state
    // precisely BECAUSE the outbox is a single global queue, so they cannot be
    // independent tests.
    client
        .execute("DELETE FROM webhook_deliveries", &[])
        .await
        .unwrap();
    client.execute("DELETE FROM webhooks", &[]).await.unwrap();

    pg::create_webhook(
        &client,
        &org_id,
        &format!("http://{addr}/flaky"),
        &serde_json::json!(["job.succeeded"]),
        "flaky-secret",
    )
    .await
    .unwrap();
    pg::create_webhook(
        &client,
        &org_id,
        &format!("http://{addr}/nope"),
        &serde_json::json!(["job.succeeded"]),
        "nope-secret",
    )
    .await
    .unwrap();

    let queued = webhook::emit(
        &client,
        &org_id,
        "acme",
        "job.succeeded",
        &serde_json::json!({ "job_id": "01RETRY0000000000000000000", "kind": "run_all_full", "state": "succeeded" }),
    )
    .await
    .unwrap();
    assert_eq!(queued.len(), 2, "both subscribers are queued");

    // First pass: the flaky one 503s (retryable -> back to pending behind a
    // backoff) and the 400 one fails outright.
    for _ in 0..2 {
        webhook::drain_once_with_egress(&client, &http, Egress::Unguarded)
            .await
            .unwrap()
            .expect("a delivery was due");
    }

    // A plain async fn, not a closure: an async closure capturing `client`
    // moves it on the first call.
    async fn log(
        client: &tokio_postgres::Client,
        org_id: &str,
        state: Option<&str>,
    ) -> Vec<er_entities::webhook_delivery::WebhookDelivery> {
        er_entities::webhook_delivery::read::page(client, org_id, None, state, 50)
            .await
            .unwrap()
    }

    // The 400 is terminal after ONE attempt: a subscriber rejecting the request
    // itself will reject it identically five more times, so spending the
    // attempts is pure waste. Distinguishing this from a 503 is the whole point
    // of classifying rather than blanket-retrying.
    let failed = log(&client, &org_id, Some("failed")).await;
    assert_eq!(failed.len(), 1, "exactly one terminal failure");
    assert_eq!(failed[0].response_code, Some(400));
    assert_eq!(failed[0].attempt, 1, "a 4xx must not consume retries");

    // The 503 went back to pending, with a backoff in the future — which is
    // also why the next drain finds nothing: the delivery is not due yet.
    let pending = log(&client, &org_id, Some("pending")).await;
    assert_eq!(pending.len(), 1, "the 503 is queued for another attempt");
    assert_eq!(pending[0].attempt, 1);
    assert!(
        pending[0].next_attempt_at.is_some(),
        "a retry must be scheduled, not immediate — otherwise a dead endpoint \
         spins the drain loop"
    );
    assert!(
        webhook::drain_once_with_egress(&client, &http, Egress::Unguarded)
            .await
            .unwrap()
            .is_none(),
        "nothing is due until the backoff elapses"
    );

    // Clear the backoff to simulate time passing, rather than sleeping: the
    // backoff curve is asserted by its own column, and a test that waits 5s per
    // attempt would be 15s of nothing.
    let pending_id = pending[0].id.clone();
    client
        .execute(
            "UPDATE webhook_deliveries SET next_attempt_at = now() - interval '1 second' \
             WHERE id = $1",
            &[&pending_id],
        )
        .await
        .unwrap();

    // Second attempt: 503 again, still retryable.
    let second = webhook::drain_once_with_egress(&client, &http, Egress::Unguarded)
        .await
        .unwrap()
        .expect("the retry is due");
    assert_eq!(second.state, "pending", "still retrying after two failures");
    assert_eq!(second.status, Some(503));

    client
        .execute(
            "UPDATE webhook_deliveries SET next_attempt_at = now() - interval '1 second' \
             WHERE id = $1",
            &[&pending_id],
        )
        .await
        .unwrap();

    // Third attempt: the subscriber is back, and the event it missed twice
    // arrives. This is the behaviour the inline-and-discard path could not have.
    let third = webhook::drain_once_with_egress(&client, &http, Egress::Unguarded)
        .await
        .unwrap()
        .expect("the second retry is due");
    assert_eq!(third.state, "delivered", "{:?}", third.error);
    assert_eq!(third.status, Some(200));

    let delivered = log(&client, &org_id, Some("delivered")).await;
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].attempt, 3, "two failures then a success");
    assert!(delivered[0].delivered_at.is_some());
    assert_eq!(
        delivered[0].error, None,
        "a success must clear the previous attempt's error, or the log reads as \
         failed forever"
    );

    // The retried body is byte-identical to the first attempt's. It has to be:
    // the signature is computed over these bytes, so a rebuilt payload would
    // fail the subscriber's verification on exactly the deliveries that needed
    // retrying.
    let bodies = flaky_state.bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 3, "three attempts reached the listener");
    assert_eq!(bodies[0], bodies[1], "attempt 2 resent the same bytes");
    assert_eq!(bodies[1], bodies[2], "attempt 3 resent the same bytes");

    // And nothing is left owing.
    assert!(
        webhook::drain_once_with_egress(&client, &http, Egress::Unguarded)
            .await
            .unwrap()
            .is_none(),
        "the outbox is drained"
    );
}
