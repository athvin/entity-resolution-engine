//! The scheduler tick over real Postgres (behind `ER_TEST_PG_DSN`).
//!
//! `schedules` shipped as inert storage: the table, its CRUD and its cron
//! validation all existed, but nothing ever read a row to enqueue anything and
//! `last_enqueued_at` was never written. Every schedule in the product — and
//! `config.correction_pass.cadence` with them — simply never fired. These tests
//! drive `dispatcher::schedule_tick` directly, with the clock as a parameter, so
//! firing behaviour is asserted rather than waited for.
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_scheduler_tick

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, dispatcher, pg};
use tower::ServiceExt;

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

fn at(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .unwrap()
        .and_utc()
}

/// Jobs for an org, newest first.
async fn jobs_for(app: &axum::Router, org: &str, bearer: &str) -> Vec<serde_json::Value> {
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/v1/orgs/{org}/jobs"),
            bearer,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    body["jobs"]
        .as_array()
        .cloned()
        .expect("GET /jobs returns {\"jobs\": [...]}")
}

async fn schedules(app: &axum::Router, bearer: &str) -> Vec<serde_json::Value> {
    let resp = app
        .clone()
        .oneshot(req("GET", "/v1/orgs/acme/schedules", bearer, Body::empty()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    json_body(resp).await.as_array().cloned().unwrap()
}

#[tokio::test]
async fn schedules_actually_fire_and_fire_once_per_minute() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres scheduler tick (set ER_TEST_PG_DSN to run)");
        return;
    };

    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS staged_steward_actions, audit_log, schedules, config_versions, api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    let config_path = std::env::temp_dir().join(format!("er_tick_{}.yaml", std::process::id()));
    let config_path = config_path.to_string_lossy().to_string();
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs",
            "op-secret",
            Body::from(
                serde_json::json!({ "name": "acme", "config_path": config_path }).to_string(),
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

    // An empty control plane must tick cleanly.
    assert_eq!(
        dispatcher::schedule_tick(&client, at("2026-10-20 03:00:00"))
            .await
            .unwrap(),
        0,
        "no schedules -> nothing enqueued"
    );

    // A schedule that fires at 03:00 on Sundays — the correction-pass cadence.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/schedules",
            &admin_key,
            Body::from(r#"{"kind":"correct","cron":"0 3 * * 0"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let schedule_id = json_body(resp).await["id"].as_str().unwrap().to_string();

    // A minute that does not match fires nothing.
    let monday = at("2026-10-19 03:00:00");
    assert_eq!(
        dispatcher::schedule_tick(&client, monday).await.unwrap(),
        0,
        "2026-10-19 is a Monday; a Sunday schedule must not fire"
    );
    assert_eq!(jobs_for(&app, "acme", &admin_key).await.len(), 0);

    // The matching minute fires exactly one job, attributed to its schedule.
    let sunday = at("2026-10-18 03:00:00");
    assert_eq!(
        dispatcher::schedule_tick(&client, sunday).await.unwrap(),
        1,
        "2026-10-18 03:00 is Sunday 03:00; the schedule must fire"
    );
    let fired = jobs_for(&app, "acme", &admin_key).await;
    assert_eq!(fired.len(), 1);
    assert_eq!(fired[0]["kind"], "correct");
    assert_eq!(
        fired[0]["schedule_id"], schedule_id,
        "a scheduled run must name the schedule that caused it"
    );
    assert_eq!(fired[0]["created_by"], "scheduler");
    assert_eq!(
        fired[0]["idempotency_key"],
        format!("sched:{schedule_id}:202610180300")
    );

    // last_enqueued_at is now visible to an operator.
    let rows = schedules(&app, &admin_key).await;
    assert!(
        !rows[0]["last_enqueued_at"].is_null(),
        "the tick must stamp last_enqueued_at"
    );

    // Ticking the SAME minute again is a no-op — this is what makes two
    // dispatcher replicas safe, and it is the idempotency key that does it.
    assert_eq!(
        dispatcher::schedule_tick(&client, sunday).await.unwrap(),
        0,
        "a second tick in the same minute must not enqueue again"
    );
    assert_eq!(
        jobs_for(&app, "acme", &admin_key).await.len(),
        1,
        "still exactly one job after a repeat tick"
    );

    // A later matching minute is a new firing.
    assert_eq!(
        dispatcher::schedule_tick(&client, at("2026-10-25 03:00:00"))
            .await
            .unwrap(),
        1,
        "the next Sunday is a separate firing"
    );
    assert_eq!(jobs_for(&app, "acme", &admin_key).await.len(), 2);

    // No catch-up: skipping over many missed firings enqueues one job for the
    // minute we are actually in, not one per minute since the last run.
    assert_eq!(
        dispatcher::schedule_tick(&client, at("2026-11-29 03:00:00"))
            .await
            .unwrap(),
        1,
        "a long gap must not backfill every missed firing"
    );
    assert_eq!(jobs_for(&app, "acme", &admin_key).await.len(), 3);

    // Disabling stops it.
    let resp = app
        .clone()
        .oneshot(req(
            "PATCH",
            &format!("/v1/orgs/acme/schedules/{schedule_id}"),
            &admin_key,
            Body::from(r#"{"enabled":false}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        dispatcher::schedule_tick(&client, at("2026-12-06 03:00:00"))
            .await
            .unwrap(),
        0,
        "a disabled schedule must not fire"
    );

    // Re-enable, then suspend the org: a suspended tenant accrues no runs.
    let resp = app
        .clone()
        .oneshot(req(
            "PATCH",
            &format!("/v1/orgs/acme/schedules/{schedule_id}"),
            &admin_key,
            Body::from(r#"{"enabled":true}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/suspend",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        dispatcher::schedule_tick(&client, at("2026-12-13 03:00:00"))
            .await
            .unwrap(),
        0,
        "a suspended org must not accumulate scheduled runs"
    );

    // ---- a second schedule on a second org: */15 fires four times an hour ----
    //
    // Same test, not a second #[tokio::test]: `schedule_tick` is global across
    // tenants, so two tests ticking concurrently would each count the other's
    // firings.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs",
            "op-secret",
            Body::from(
                serde_json::json!({ "name": "beta", "config_path": config_path }).to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/beta/api-keys",
            "op-secret",
            Body::from(r#"{"role":"admin"}"#),
        ))
        .await
        .unwrap();
    let beta_key = json_body(resp).await["key"].as_str().unwrap().to_string();
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/beta/schedules",
            &beta_key,
            Body::from(r#"{"kind":"lake_maintain","cron":"*/15 * * * *"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // acme is suspended by now, so every firing in this sweep is beta's.
    let mut fired = 0usize;
    for minute in 0..60u32 {
        let now = at(&format!("2026-10-20 08:{minute:02}:00"));
        fired += dispatcher::schedule_tick(&client, now).await.unwrap();
    }
    assert_eq!(fired, 4, "*/15 must fire four times in an hour");
    let beta_jobs = jobs_for(&app, "beta", &beta_key).await;
    assert_eq!(beta_jobs.len(), 4);
    assert!(beta_jobs.iter().all(|j| j["kind"] == "lake_maintain"));

    let _ = std::fs::remove_file(&config_path);
}
