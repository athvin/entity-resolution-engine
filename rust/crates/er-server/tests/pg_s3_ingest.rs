//! The Postgres half of S3 auto-ingestion (E9c), behind `ER_TEST_PG_DSN`: the
//! two org columns round-trip through `POST /v1/orgs`, the poller's sweep read
//! selects the right tenants, and the cursor upserts rather than stacking.
//!
//! **What this does NOT cover, and why.** The listing and download half needs a
//! bucket, so nothing here exercises `Watcher`. That split is deliberate rather
//! than a gap papered over: the rules that decide which of a tenant's files
//! gets read — classification, the newest-with-tie-break choice, the
//! path-escape refusal, and the key-plus-ETag freshness test — are pure
//! functions with unit tests in `src/s3_ingest.rs`, precisely so they are not
//! hostage to a credential. What is left needing Postgres is the state: the
//! columns, the sweep predicate, and the cursor. Those are what this file is
//! for. A green run here is **not** evidence that an object can be ingested
//! end to end; that wants a bucket and a live dev tenant.
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5466 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_s3_ingest

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

use crate::common;

fn req(method: &str, uri: &str, bearer: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .unwrap()
}

/// One test with phases: the Postgres tier shares one control-plane schema, so
/// two tests that each drop it deadlock through the `orgs_history` trigger
/// (`pg_serial.rs` enforces this).
#[tokio::test]
async fn s3_ingest_columns_sweep_and_cursor() {
    let Some(db) = common::fresh_db("s3_ingest").await else {
        return;
    };
    let dsn = db.dsn().to_string();

    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    // --- phase 1: the two columns round-trip through the create endpoint.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs",
            "op-secret",
            Body::from(
                r#"{"name":"watched","config_path":"/configs/watched.yaml","env":{},
                    "drop_root":"/tmp/er-s3-watched",
                    "s3_ingest_bucket":"tenant-drops","s3_ingest_prefix":"tenants/watched"}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let watched = pg::get_org(&client, "watched")
        .await
        .unwrap()
        .expect("watched exists");
    assert_eq!(watched.s3_ingest_bucket.as_deref(), Some("tenant-drops"));
    assert_eq!(
        watched.s3_ingest_prefix.as_deref(),
        Some("tenants/watched"),
        "a prefix accepted by the API and not persisted would leave the poller \
         reading the bucket root"
    );

    // A tenant that did not ask for auto-ingest. Absent fields, not empty
    // strings: this is every existing tenant, and the poller must not touch it.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs",
            "op-secret",
            Body::from(r#"{"name":"unwatched","config_path":"/configs/unwatched.yaml","env":{}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let unwatched = pg::get_org(&client, "unwatched").await.unwrap().unwrap();
    assert!(
        unwatched.s3_ingest_bucket.is_none(),
        "auto-ingest must stay off unless a tenant opted in"
    );

    // --- phase 2: the sweep read selects only tenants with a bucket.
    let swept = er_entities::org::read::with_s3_ingest(&client)
        .await
        .unwrap();
    assert_eq!(
        swept.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(),
        vec!["watched"],
        "the poll must cost nothing on a deployment where nobody has turned \
         auto-ingest on"
    );

    // A suspended tenant drops out of the sweep. `delivery_preconditions`
    // refuses it anyway, so this is the second of two guards — but the first
    // one costs a listing per source before refusing, and a suspended tenant
    // should not be billed S3 requests for imports that cannot land.
    assert!(
        pg::set_org_state(&client, "watched", "suspended", "active")
            .await
            .unwrap()
    );
    assert!(
        er_entities::org::read::with_s3_ingest(&client)
            .await
            .unwrap()
            .is_empty(),
        "a suspended tenant must not be polled"
    );
    assert!(
        pg::set_org_state(&client, "watched", "active", "suspended")
            .await
            .unwrap()
    );

    // --- phase 3: the cursor corrects rather than stacking.
    let org_id = watched.id.clone();
    // Cursors are the dataset's now; the fixture uses the same default
    // dataset the poller resolves.
    let ds = pg::default_dataset_id(&client, &org_id)
        .await
        .unwrap()
        .expect("the org's default dataset");
    er_entities::s3_ingest_cursor::create::advance(
        &client,
        &er_ids::new_v7(),
        &org_id,
        Some(&ds),
        "crm",
        "tenants/watched/crm/day1.csv",
        Some("etag-1"),
        10,
        "delivery-1",
    )
    .await
    .unwrap();
    let first = er_entities::s3_ingest_cursor::read::by_org(&client, &ds)
        .await
        .unwrap();
    assert_eq!(first.len(), 1);
    let first_created = first[0].created_at;

    er_entities::s3_ingest_cursor::create::advance(
        &client,
        &er_ids::new_v7(),
        &org_id,
        Some(&ds),
        "crm",
        "tenants/watched/crm/day2.csv",
        Some("etag-2"),
        20,
        "delivery-2",
    )
    .await
    .unwrap();
    let rows = er_entities::s3_ingest_cursor::read::by_org(&client, &ds)
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "two rows for one source would make 'what have we already read' \
         ambiguous, with no principled way to pick"
    );
    assert_eq!(rows[0].object_key, "tenants/watched/crm/day2.csv");
    assert_eq!(rows[0].etag.as_deref(), Some("etag-2"));
    assert_eq!(rows[0].size_bytes, 20);
    assert_eq!(rows[0].delivery_id, "delivery-2");
    assert_eq!(
        rows[0].created_at, first_created,
        "when this source was first auto-ingested stays recoverable"
    );
    assert!(
        rows[0].updated_at >= first_created,
        "the correction advances updated_at"
    );

    // A second source is a second row, not a replacement: the natural key is
    // (org, source), and collapsing it to (org) would mean one source's drop
    // marked every other source as read.
    er_entities::s3_ingest_cursor::create::advance(
        &client,
        &er_ids::new_v7(),
        &org_id,
        Some(&ds),
        "billing",
        "tenants/watched/billing/day1.csv",
        None,
        5,
        "delivery-3",
    )
    .await
    .unwrap();
    let rows = er_entities::s3_ingest_cursor::read::by_org(&client, &ds)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter().map(|r| r.source.as_str()).collect::<Vec<_>>(),
        vec!["billing", "crm"],
        "ordered, so the sweep's behaviour does not depend on insertion order"
    );
    assert!(
        rows.iter()
            .find(|r| r.source == "billing")
            .unwrap()
            .etag
            .is_none(),
        "a listing that omits the ETag must still produce a usable cursor"
    );

    // --- phase 4: the read is tenant-scoped.
    let other_id = pg::org_id_by_name(&client, "unwatched")
        .await
        .unwrap()
        .unwrap();
    assert!(
        er_entities::s3_ingest_cursor::read::by_org(&client, &other_id)
            .await
            .unwrap()
            .is_empty(),
        "one tenant's ingest history must not be visible under another's id"
    );

    // --- phase 5: the role boundary on the create route that carries these
    // fields. A non-operator must not be able to point the poller at a bucket.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs",
            "not-the-operator",
            Body::from(
                r#"{"name":"sneaky","config_path":"/c.yaml","env":{},
                    "s3_ingest_bucket":"someone-elses-bucket"}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "configuring which bucket we read is an operator action"
    );
    assert!(
        pg::get_org(&client, "sneaky").await.unwrap().is_none(),
        "the refused call must not have created the org"
    );
}
