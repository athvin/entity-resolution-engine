//! The audit ledger's E4 semantics over the real Postgres backend (behind
//! `ER_TEST_PG_DSN`): the feed's actor and time-window filters, the CSV export
//! arm (with its formula guard), and the retention sweep — old audit rows and
//! settled old deliveries retired, pending deliveries never, the compliance
//! floor clamped. ONE test with sequential phases (the `pg_serial.rs` rule).
//!
//! Aging is done by UPDATEing `at`/`created_at` directly: `append.sql` stamps
//! `now()` by design, and a test that cannot move the clock moves the rows.
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test main pg_audit

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use serde_json::json;
use tower::ServiceExt;

use crate::common;

fn req(method: &str, uri: &str, bearer: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .body(Body::empty())
        .unwrap()
}

async fn body_text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn audit_filters_export_and_retention() {
    let Some(db) = common::fresh_db("audit").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::ensure_org(&client, "acme", "/configs/acme.yaml")
        .await
        .unwrap();
    let org_id = pg::org_id_by_name(&client, "acme")
        .await
        .unwrap()
        .expect("acme exists");
    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    // ---- seed: three actors, one formula-shaped, distinct actions ----------
    for (actor, action) in [
        ("scheduler", "job.submit"),
        ("sf-extract", "import.extract"),
        ("=2+5", "webhook.create"),
    ] {
        pg::audit(&client, actor, Some("acme"), None, action, &json!({"k": 1}))
            .await
            .unwrap();
    }

    // ---- the actor filter narrows to that actor's rows ---------------------
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/audit?actor=sf-extract",
            "op-secret",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let rows: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1, "one sf-extract row was seeded");
    assert!(
        rows.iter().all(|r| r["actor"] == "sf-extract"),
        "every row is the filtered actor: {rows:?}"
    );
    assert!(
        rows.iter().all(|r| r.get("dataset").is_some()),
        "the projection carries the dataset key again"
    );

    // ---- the [since, until) window is half-open ----------------------------
    // Age the scheduler row to a known instant and window around it.
    client
        .execute(
            "UPDATE public.audit_log SET at = '2026-01-15T12:00:00Z' WHERE actor = 'scheduler'",
            &[],
        )
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/audit?since=2026-01-01T00:00:00Z&until=2026-02-01T00:00:00Z",
            "op-secret",
        ))
        .await
        .unwrap();
    let rows: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert_eq!(
        rows.as_array().unwrap().len(),
        1,
        "only the aged row falls in January"
    );
    assert_eq!(rows[0]["actor"], "scheduler");
    // The upper bound is exclusive: a window ending exactly at the row's
    // instant does not include it.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/audit?since=2026-01-01T00:00:00Z&until=2026-01-15T12:00:00Z",
            "op-secret",
        ))
        .await
        .unwrap();
    let rows: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert!(
        rows.as_array().unwrap().is_empty(),
        "the window is half-open: [since, until)"
    );

    // ---- the CSV export: same page, text/csv, formula-guarded --------------
    let resp = app
        .clone()
        .oneshot(req("GET", "/v1/orgs/acme/audit?format=csv", "op-secret"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("text/csv; charset=utf-8")
    );
    let csv = body_text(resp).await;
    assert!(csv.starts_with("id,at,actor,org,dataset,action,trace_id,detail\n"));
    assert!(
        csv.contains(",'=2+5,"),
        "a formula-shaped actor is guarded with a leading quote: {csv}"
    );
    assert!(csv.contains("sf-extract"), "{csv}");

    // The operator feed exports too.
    let resp = app
        .clone()
        .oneshot(req("GET", "/v1/audit?format=csv&org=acme", "op-secret"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("text/csv; charset=utf-8")
    );

    // ---- retention: expired audit rows retire, in-window rows stay ---------
    // Age one row past the 400-day floor; the January row (this year) stays.
    client
        .execute(
            "UPDATE public.audit_log SET at = now() - interval '401 days'
             WHERE actor = '=2+5'",
            &[],
        )
        .await
        .unwrap();
    // Deliveries: one settled-and-expired (must go), one pending-and-expired
    // (must STAY — an undelivered webhook is work, not history). `emit` with a
    // subscribe-all webhook writes the pending rows.
    pg::create_webhook(
        &client,
        &org_id,
        None,
        "https://hooks.example/acme",
        &json!([]),
        "s3cret",
    )
    .await
    .unwrap();
    for _ in 0..2 {
        er_server::webhook::emit(
            &client,
            &org_id,
            None,
            "acme",
            "job.succeeded",
            &json!({"n": 1}),
        )
        .await
        .unwrap();
    }
    client
        .execute(
            "UPDATE public.webhook_deliveries SET created_at = now() - interval '401 days'",
            &[],
        )
        .await
        .unwrap();
    client
        .execute(
            "UPDATE public.webhook_deliveries SET state = 'delivered'
             WHERE id IN (SELECT id FROM public.webhook_deliveries LIMIT 1)",
            &[],
        )
        .await
        .unwrap();

    let (audit_purged, deliveries_purged) = pg::purge_expired(&client).await.unwrap();
    assert_eq!(audit_purged, 1, "exactly the 401-day-old audit row retired");
    assert_eq!(
        deliveries_purged, 1,
        "the settled expired delivery retired; the pending one did not"
    );
    let remaining: i64 = client
        .query_one("SELECT count(*) FROM public.audit_log", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(remaining, 2, "the in-window audit rows survive");
    let pending: i64 = client
        .query_one(
            "SELECT count(*) FROM public.webhook_deliveries WHERE state = 'pending'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(pending, 1, "the expired-but-pending delivery is still work");

    // A second sweep finds nothing — the purge is convergent, not repeating.
    assert_eq!(pg::purge_expired(&client).await.unwrap(), (0, 0));
}
