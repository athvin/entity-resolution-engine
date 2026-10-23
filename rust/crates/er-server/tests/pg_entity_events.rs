//! Entity-level webhook events and the detail endpoint behind them (O3c-2),
//! gated on `ER_TEST_PG_DSN` — and, for the phases that need a real run,
//! `DEDUPE_E2E_DUCKLAKE=1` as well.
//!
//! O3a deliberately catalogued only the three terminal `job.*` outcomes and
//! said why entity events were not among them: "thousands of un-retried POSTs
//! per run would be worse than none." O3b built the outbox, which fixes the
//! *retry* half and leaves the volume exactly where it was — a 10M-row corpus
//! still writes millions of ledger rows.
//!
//! So the design is an aggregate: at most one event per KIND per run, carrying
//! a count and a `detail_path`. That trade is only honest if two things hold,
//! and they are what this file asserts:
//!
//!   * the fan-out really is bounded — the ledger wrote strictly more rows than
//!     the number of deliveries, and no kind delivered twice;
//!   * nothing was lost — the sum of the delivered counts equals the number of
//!     rows the detail endpoint serves for that run. An aggregate that cannot
//!     be expanded back is a summary, not a notification.
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5466 user=er password=er dbname=er' \
//!     DEDUPE_E2E_DUCKLAKE=1 cargo test -p er-server --test pg_entity_events

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

const TEST_YAML: &str = include_str!("../../er-cli/tests/fixtures/test.yaml");

fn repo_fixture(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/static")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

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
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn entity_events_are_aggregated_per_kind_and_expand_through_the_detail_endpoint() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping entity-event webhooks (set ER_TEST_PG_DSN to run)");
        return;
    };

    // ---- setup: a clean control plane, two orgs, keys ----------------------
    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS webhook_deliveries, matching_models, staged_steward_actions, \
             audit_log, webhooks, schedules, config_versions, api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    let tmp = std::env::temp_dir().join(format!("er_entity_events_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    std::fs::create_dir_all(tmp.join("drop")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();
    let drop_root = tmp.join("drop").to_string_lossy().into_owned();
    let config_path = tmp.join("acme.yaml");
    std::fs::write(&config_path, TEST_YAML).unwrap();
    for source in ["crm", "billing", "webforms"] {
        std::fs::copy(
            repo_fixture(&format!("base_10/base/{source}.csv")),
            tmp.join("drop").join(format!("{source}.csv")),
        )
        .unwrap();
    }

    let env = serde_json::json!({
        "ER_LAKE_CATALOG": catalog,
        "ER_LAKE_DATA_PATH": data_path,
        "ER_CONFIG_PATH": config_path.to_string_lossy(),
        "ER_MODEL_PATH": repo_fixture("model_test_v1.json"),
        "ER_TF_PATH": repo_fixture("model_test_v1.tf.csv"),
        // The frozen snapshot's own identity. Without it the default "tf"
        // matches no row in the fixture and the run refuses to score rather
        // than silently dropping every TF adjustment.
        "ER_TF_SNAPSHOT_ID": "01JWMDTFSNAP00000000000001",
    });
    pg::upsert_org_byo(
        &client,
        "acme",
        config_path.to_str().unwrap(),
        &env,
        Some(&drop_root),
    )
    .await
    .unwrap();
    // A second tenant, for the role boundary. It gets no lake: the point is
    // that its key is refused before anything is opened.
    pg::upsert_org_byo(
        &client,
        "other",
        "/configs/other.yaml",
        &serde_json::json!({}),
        None,
    )
    .await
    .unwrap();
    let org_id = pg::org_id_by_name(&client, "acme")
        .await
        .unwrap()
        .expect("acme exists");

    let api = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let key = |org: &'static str, role: &'static str| {
        let api = api.clone();
        async move {
            let resp = api
                .oneshot(req(
                    "POST",
                    &format!("/v1/orgs/{org}/api-keys"),
                    "op-secret",
                    Body::from(format!(r#"{{"role":"{role}"}}"#)),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::CREATED);
            json_body(resp).await["key"].as_str().unwrap().to_string()
        }
    };
    let admin_key = key("acme", "admin").await;
    let viewer_key = key("acme", "viewer").await;
    let other_key = key("other", "admin").await;

    // ---- phase 1: the six entity events are subscribable -------------------
    //
    // Registration validates against the catalog, so this request is the proof
    // the catalog actually gained them — before O3c-2 every one of these was a
    // 422 naming the events that did exist.
    let resp = api
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/webhooks",
            &admin_key,
            Body::from(
                serde_json::json!({
                    "url": "https://example.test/entities",
                    "events": [
                        "entity.created", "entity.member_added", "entity.member_removed",
                        "entity.merged", "entity.split", "entity.retired",
                    ],
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "the entity events must be subscribable or the aggregate reaches nobody"
    );

    // ---- phase 2: the detail endpoint's role boundary ----------------------
    //
    // Checked before the lake phases so it is asserted even where DuckDB's
    // extensions are unavailable: an authorization bug must not be able to hide
    // behind a skipped tier.
    let detail = "/v1/orgs/acme/runs/01ANYRUN0000000000000000000/entity-events";
    let resp = api
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(detail)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "an unauthenticated read of another tenant's ledger must be refused"
    );
    let resp = api
        .clone()
        .oneshot(req("GET", detail, &other_key, Body::empty()))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "an admin of another org must not read acme's entity events — and 404 \
         rather than 403, because 403 would confirm the tenant exists"
    );
    // The floor is `viewer`, the lowest role there is, so every role INSIDE the
    // org passes — which is the right answer for a read of the org's own
    // ledger, and is proved below by the viewer key serving the detail page.

    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping the run phases (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        let _ = std::fs::remove_dir_all(&tmp);
        return;
    }

    // ---- phase 3: a real run emits one aggregate per kind ------------------
    let queued = pg::enqueue(&client, &org_id, "run_all_full", 0, None, None, None)
        .await
        .unwrap();
    let ran = er_server::dispatcher::dispatch_once_with(
        &client,
        &Ok(None),
        &er_server::mail::Mailer::off(),
    )
    .await
    .unwrap();
    assert!(ran, "the queued run should have been claimed");
    let job = pg::get_job(&client, &org_id, &queued.id)
        .await
        .unwrap()
        .expect("job present");
    assert_eq!(
        job.state, "succeeded",
        "the run must succeed for its aggregate to mean anything; error: {:?}",
        job.error_detail
    );

    let deliveries = er_entities::webhook_delivery::read::page(&client, &org_id, None, None, 200)
        .await
        .unwrap();
    let entity_deliveries: Vec<_> = deliveries
        .iter()
        .filter(|d| d.event_type.starts_with("entity."))
        .collect();
    assert!(
        !entity_deliveries.is_empty(),
        "a run that resolved a corpus must announce what it did to it"
    );

    // One per KIND, and no kind twice. This is the bounded-fan-out claim
    // stated as an assertion rather than as a comment.
    let mut kinds: Vec<&str> = entity_deliveries
        .iter()
        .map(|d| d.event_type.as_str())
        .collect();
    kinds.sort_unstable();
    let mut unique = kinds.clone();
    unique.dedup();
    assert_eq!(
        kinds, unique,
        "a kind delivered twice means the aggregate is per-row after all: {kinds:?}"
    );
    assert!(
        entity_deliveries.len() <= 6,
        "at most one event per catalogued kind, got {}",
        entity_deliveries.len()
    );

    // base_10 resolves to 10 entities, and a first full run creates all of them.
    let created = entity_deliveries
        .iter()
        .find(|d| d.event_type == "entity.created")
        .expect("a first run creates entities");
    assert_eq!(
        created.payload["count"], 10,
        "base_10 resolves to 10 entities, so that is what `created` must count"
    );
    assert_eq!(
        created.payload["run_id"], queued.id,
        "a subscriber correlates the aggregate with the job through this id"
    );
    assert_eq!(created.payload["event_type"], "created");
    assert_eq!(
        created.payload["detail_path"],
        serde_json::Value::String(format!(
            "/v1/orgs/acme/runs/{}/entity-events?event_type=created",
            queued.id
        )),
        "the aggregate has to say where the rows are, already narrowed to its kind"
    );
    assert_eq!(
        created.state, "pending",
        "emit records, the drain delivers — the outbox property O3b established"
    );

    // The fan-out really was reduced: more ledger rows than deliveries. With
    // base_10 the margin is small; the point is the direction, which is what
    // scales to a corpus where one POST per row is the failure O3a named.
    let ledger_rows: i64 = {
        // Scoped so the read connection is closed before the next run claims
        // the lake: a lingering attach on the catalog file is exactly the kind
        // of flake that reads as a product bug.
        let session = er_cli::open_lake(&catalog, &data_path).unwrap();
        session
            .query_strings(&format!(
                "select count(*) from lake.main.entity_events where run_id = '{}'",
                queued.id
            ))
            .unwrap()[0][0]
            .parse()
            .unwrap()
    };
    assert!(
        ledger_rows > entity_deliveries.len() as i64,
        "{ledger_rows} ledger rows collapsed into {} deliveries — if these are equal the \
         aggregate is not aggregating",
        entity_deliveries.len()
    );

    // ---- phase 3b: a SECOND run, producing more than one kind --------------
    //
    // A first full run over base_10 writes nothing but `created` — every entity
    // is new — so a one-kind run cannot tell a per-kind aggregate from a
    // per-run one, and cannot exercise the kind filter at all. Two rows
    // appended to the crm delivery fix that: one is a near-duplicate of an
    // existing person (it joins that entity) and one is a stranger (it makes a
    // new one), so the run moves the partition in two directions at once.
    let crm = tmp.join("drop").join("crm.csv");
    let mut appended = std::fs::read_to_string(&crm).unwrap();
    if !appended.ends_with('\n') {
        appended.push('\n');
    }
    appended.push_str(
        "C900,Robert,Chen,robert.chen@example.com,(628) 555-0101,1420 Judah Street Apt 5,\
         San Francisco,CA,94107,1985-03-14,2024-03-01 09:00:00,P1\n\
         C901,Ingrid,Halvorsen,ingrid.halvorsen@example.com,(503) 555-0909,77 Alder Way,\
         Portland,OR,97209,1972-05-18,2024-03-01 09:30:00,P99\n",
    );
    std::fs::write(&crm, appended).unwrap();
    let second = pg::enqueue(&client, &org_id, "run_all_full", 0, None, None, None)
        .await
        .unwrap();
    assert!(
        er_server::dispatcher::dispatch_once_with(
            &client,
            &Ok(None),
            &er_server::mail::Mailer::off()
        )
        .await
        .unwrap()
    );
    let second_job = pg::get_job(&client, &org_id, &second.id)
        .await
        .unwrap()
        .expect("second job present");
    assert_eq!(
        second_job.state, "succeeded",
        "second run failed: {:?}",
        second_job.error_detail
    );
    let second_deliveries: Vec<_> =
        er_entities::webhook_delivery::read::page(&client, &org_id, None, None, 200)
            .await
            .unwrap()
            .into_iter()
            .filter(|d| d.event_type.starts_with("entity.") && d.payload["run_id"] == second.id)
            .collect();
    let mut second_kinds: Vec<(&str, i64)> = second_deliveries
        .iter()
        .map(|d| {
            (
                d.event_type.as_str(),
                d.payload["count"].as_i64().unwrap_or(-1),
            )
        })
        .collect();
    second_kinds.sort_unstable();
    assert_eq!(
        second_kinds,
        vec![("entity.created", 1), ("entity.member_added", 1)],
        "the stranger makes one entity and the near-duplicate joins one, so the run \
         announces exactly those two things once each"
    );

    // ---- phase 4: the detail endpoint expands the aggregate back -----------
    //
    // Fetched the way a subscriber would: follow the `detail_path` the
    // aggregate handed over, which is already narrowed to its kind. Checked on
    // the TWO-kind run, because on a run whose ledger holds one kind a filter
    // that is ignored is indistinguishable from one that works.
    let page_for = |path: &str| {
        let api = api.clone();
        let viewer = viewer_key.clone();
        let path = path.to_string();
        async move {
            let resp = api
                .oneshot(req("GET", &path, &viewer, Body::empty()))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "GET {path}");
            json_body(resp).await
        }
    };
    for (kind, delivered) in &second_kinds {
        let detail_path = second_deliveries
            .iter()
            .find(|d| d.event_type == *kind)
            .unwrap()
            .payload["detail_path"]
            .as_str()
            .unwrap()
            .to_string();
        let page = page_for(&detail_path).await;
        let items = page["items"].as_array().unwrap();
        assert_eq!(
            items.len() as i64,
            *delivered,
            "{kind}'s detail must serve exactly the rows its count claimed"
        );
        let bare = kind.strip_prefix("entity.").unwrap();
        for item in items {
            assert_eq!(item["event_type"], bare, "the kind filter must hold");
            assert_eq!(item["run_id"], second.id);
        }
        assert!(
            page["snapshot"].as_i64().is_some(),
            "a read-API page names the snapshot it was pinned to"
        );
    }
    // ...and the unfiltered page over the same run is strictly larger, which is
    // what makes the two assertions above a test of the filter rather than of
    // the run having produced one kind.
    let whole = page_for(&format!("/v1/orgs/acme/runs/{}/entity-events", second.id)).await;
    assert_eq!(
        whole["items"].as_array().unwrap().len(),
        2,
        "both kinds, unfiltered"
    );

    // The first run's page: every entity it created, in one page.
    let page = page_for(&format!(
        "/v1/orgs/acme/runs/{}/entity-events?event_type=created",
        queued.id
    ))
    .await;
    assert_eq!(
        page["items"].as_array().unwrap().len(),
        10,
        "the detail must serve exactly the rows the aggregate counted"
    );

    // Losslessness: every row the aggregate counted is retrievable. An
    // aggregate that cannot be expanded is a summary, and summarising is not
    // what a webhook is for.
    let delivered_total: i64 = entity_deliveries
        .iter()
        .map(|d| d.payload["count"].as_i64().unwrap_or(0))
        .sum();
    let resp = api
        .clone()
        .oneshot(req(
            "GET",
            &format!("/v1/orgs/acme/runs/{}/entity-events?limit=200", queued.id),
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    let all = json_body(resp).await;
    assert_eq!(
        all["items"].as_array().unwrap().len() as i64,
        delivered_total,
        "the counts delivered and the rows served must describe the same run"
    );
    assert_eq!(
        delivered_total, ledger_rows,
        "and both must equal what the ledger holds"
    );

    // Paging is a keyset on `seq`, so a page boundary neither repeats nor skips.
    let resp = api
        .clone()
        .oneshot(req(
            "GET",
            &format!("/v1/orgs/acme/runs/{}/entity-events?limit=3", queued.id),
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    let first = json_body(resp).await;
    assert_eq!(first["items"].as_array().unwrap().len(), 3);
    let cursor = first["next_cursor"]
        .as_str()
        .expect("more rows than one page means a cursor")
        .to_string();
    let resp = api
        .clone()
        .oneshot(req(
            "GET",
            &format!(
                "/v1/orgs/acme/runs/{}/entity-events?limit=3&cursor={cursor}",
                queued.id
            ),
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    let second = json_body(resp).await;
    let seqs = |p: &serde_json::Value| -> Vec<i64> {
        p["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["seq"].as_i64().unwrap())
            .collect()
    };
    let (a, b) = (seqs(&first), seqs(&second));
    assert!(
        a.last() < b.first(),
        "the second page must start after the first ends: {a:?} then {b:?}"
    );

    // A cursor this endpoint could not have minted is a client error, not an
    // empty page that looks like the end of the data.
    let resp = api
        .clone()
        .oneshot(req(
            "GET",
            &format!(
                "/v1/orgs/acme/runs/{}/entity-events?cursor=not-a-cursor",
                queued.id
            ),
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // A run that produced nothing is an empty page, not a 404: the question
    // "what did run X do" has an answer even when the answer is "nothing".
    let resp = api
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/runs/01NOSUCHRUN00000000000000/entity-events",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        json_body(resp).await["items"].as_array().unwrap().len(),
        0,
        "an unknown run is an empty ledger, not a missing resource"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
