//! The webhook event catalog: what this system emits, and the registration
//! that now checks against it (behind `ER_TEST_PG_DSN`).
//!
//! A subscription was an unvalidated free-form array. `webhooks_for_delivery`
//! merely tests whether the subscriber's `events` list contains the fired type,
//! so a tenant could subscribe to `entity.merged` — which nothing emits — or to
//! `job.suceeded` with the typo, and wait forever for a delivery that was never
//! going to arrive. Nothing told them, and nothing could: there was no list of
//! what exists.
//!
//! The catalog is only worth having if it is TRUE, so the first test here reads
//! the emit sites rather than trusting the list. That is also why `job.running`
//! is absent: the dispatcher marks a job running and never emits for it.
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_webhook_catalog

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg, webhook};
use tower::ServiceExt;

/// The server's own source, so the catalog can be checked against the code that
/// emits rather than against a comment about it.
const SERVER_SRC: &str = include_str!("../src/lib.rs");
const DISPATCHER_SRC: &str = include_str!("../src/dispatcher.rs");

fn req(method: &str, uri: &str, bearer: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
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

#[test]
fn every_catalogued_event_has_a_state_that_reaches_the_emitter() {
    // `emit_job_event` builds its type as `job.<state>`, so the catalogued
    // `job.*` events are exactly the states that reach it. Checking the source
    // keeps the catalog honest: listing an event nothing fires is the bug this
    // catalog exists to prevent, and a comment claiming otherwise would not
    // fail anything.
    let emitting_sources = format!("{SERVER_SRC}{DISPATCHER_SRC}");
    for (event, _) in webhook::EVENT_CATALOG {
        let Some(state) = event.strip_prefix("job.") else {
            // A non-job event will need its own evidence when one is added.
            continue;
        };
        assert!(
            emitting_sources.contains(&format!("\"{state}\"")),
            "the catalog lists {event:?} but no emit site mentions the state \
             {state:?} — either something must emit it or it must not be listed"
        );
    }

    // And the converse direction for the one we know is NOT emitted: the
    // dispatcher marks a job running, so the state exists, but nothing emits
    // for it — so it must stay out of the catalog.
    assert!(
        DISPATCHER_SRC.contains("\"running\"") || SERVER_SRC.contains("\"running\""),
        "precondition: 'running' is a real job state"
    );
    assert!(
        !webhook::is_known_event("job.running"),
        "job.running must not be subscribable until something emits it"
    );

    // The catalog's three entries are the three terminal outcomes.
    let events: Vec<&str> = webhook::EVENT_CATALOG.iter().map(|(e, _)| *e).collect();
    assert_eq!(events, vec!["job.succeeded", "job.failed", "job.canceled"]);
    for (_, description) in webhook::EVENT_CATALOG {
        assert!(
            description.len() > 30,
            "each event needs a description a tenant can act on"
        );
    }
}

#[tokio::test]
async fn registration_refuses_an_event_nothing_emits() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres webhook catalog (set ER_TEST_PG_DSN to run)");
        return;
    };
    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS webhook_deliveries, webhooks, staged_steward_actions, audit_log, schedules, config_versions, api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let config_path = std::env::temp_dir().join(format!("er_wh_{}.yaml", std::process::id()));
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs",
            "op-secret",
            Body::from(
                serde_json::json!({
                    "name": "acme",
                    "config_path": config_path.to_string_lossy(),
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/api-keys",
            "op-secret",
            Body::from(r#"{"role":"admin"}"#),
        ))
        .await
        .unwrap();
    let admin_key = json_body(resp).await["key"].as_str().unwrap().to_string();
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/api-keys",
            "op-secret",
            Body::from(r#"{"role":"viewer"}"#),
        ))
        .await
        .unwrap();
    let viewer_key = json_body(resp).await["key"].as_str().unwrap().to_string();

    // Discovery: a tenant can see what exists instead of guessing.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/webhooks/events",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = json_body(resp).await;
    let names: Vec<&str> = listed["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["job.succeeded", "job.failed", "job.canceled"]);
    assert!(
        listed["events"][0]["description"].is_string(),
        "the catalog must explain each event, not just name it"
    );
    // The discovery list IS the list registration validates against, so the two
    // cannot disagree.
    for name in &names {
        assert!(webhook::is_known_event(name));
    }

    // A real subscription is accepted.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/webhooks",
            &admin_key,
            Body::from(
                r#"{"url":"https://example.test/hook","events":["job.failed","job.canceled"]}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // An event nothing emits is refused, and the refusal names what does exist
    // — the information that was previously unobtainable.
    for bad in [
        r#"{"url":"https://example.test/h","events":["entity.merged"]}"#,
        r#"{"url":"https://example.test/h","events":["job.suceeded"]}"#,
        r#"{"url":"https://example.test/h","events":["job.running"]}"#,
        r#"{"url":"https://example.test/h","events":["job.failed","nonsense"]}"#,
    ] {
        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                "/v1/orgs/acme/webhooks",
                &admin_key,
                Body::from(bad),
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "should refuse {bad}"
        );
        let detail = json_body(resp).await["detail"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            detail.contains("job.succeeded"),
            "the refusal must name the events that DO exist: {detail}"
        );
    }

    let _ = std::fs::remove_file(&config_path);
}
