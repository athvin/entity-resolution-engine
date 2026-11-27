//! #998: applying approved upserts to a mock Salesforce.
//!
//! Five approved upserts, one per path: a create (no record carries the
//! key; created, then asserted to its entity so extraction converges); an
//! update (the record exists and is unchanged since the proposal; its
//! values become the preimage); a stale record (modified after the
//! proposal; skipped, never written); a lost response (the upsert landed
//! but answered 500; left in flight, then verified by reading back — no
//! second write); a refusal (failed, the batch continues). Plus the
//! routes: policy validation, batch decisions, and apply's 409.
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Path as AxPath, State as AxState};
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use er_server::merge_apply::Limits;
use er_server::urlguard::Egress;
use er_server::{AppState, app, pg, upsert_apply};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::common;

#[derive(Default)]
struct Sf {
    /// external key → (id, modstamp, fields)
    by_key: BTreeMap<String, (String, String, BTreeMap<String, Value>)>,
    /// keys whose next upsert lands but answers 500
    lose: Vec<String>,
    /// keys whose upsert is refused
    refuse: Vec<String>,
    writes: Vec<String>,
    created: u32,
    base: String,
}

type Shared = Arc<Mutex<Sf>>;

async fn token(AxState(sf): AxState<Shared>) -> axum::Json<Value> {
    let base = sf.lock().unwrap().base.clone();
    axum::Json(json!({"access_token": "at", "instance_url": base, "id": ""}))
}

async fn read(
    AxState(sf): AxState<Shared>,
    AxPath((_object, _field, key)): AxPath<(String, String, String)>,
) -> axum::response::Response {
    let sf = sf.lock().unwrap();
    match sf.by_key.get(&key) {
        Some((id, stamp, fields)) => {
            let mut body = serde_json::Map::new();
            body.insert("Id".into(), json!(id));
            body.insert("SystemModstamp".into(), json!(stamp));
            for (k, v) in fields {
                body.insert(k.clone(), v.clone());
            }
            axum::Json(Value::Object(body)).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            axum::Json(json!([{"errorCode": "NOT_FOUND"}])),
        )
            .into_response(),
    }
}

async fn upsert(
    AxState(sf): AxState<Shared>,
    AxPath((_object, _field, key)): AxPath<(String, String, String)>,
    axum::Json(body): axum::Json<BTreeMap<String, Value>>,
) -> axum::response::Response {
    let mut sf = sf.lock().unwrap();
    sf.writes.push(key.clone());
    if sf.refuse.contains(&key) {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!([{"errorCode": "FIELD_INTEGRITY_EXCEPTION"}])),
        )
            .into_response();
    }
    let created = !sf.by_key.contains_key(&key);
    if created {
        sf.created += 1;
        let id = format!("003N{:011}", sf.created);
        sf.by_key.insert(
            key.clone(),
            (id, "2026-11-28T00:00:00.000+0000".into(), BTreeMap::new()),
        );
    }
    let entry = sf.by_key.get_mut(&key).unwrap();
    entry.2.extend(body);
    entry.1 = "2026-11-28T00:00:01.000+0000".into();
    let id = entry.0.clone();
    if let Some(i) = sf.lose.iter().position(|k| k == &key) {
        sf.lose.remove(i);
        return (StatusCode::INTERNAL_SERVER_ERROR, "gateway").into_response();
    }
    if created {
        (
            StatusCode::CREATED,
            axum::Json(json!({"id": id, "success": true, "created": true})),
        )
            .into_response()
    } else {
        (
            StatusCode::OK,
            axum::Json(json!({"id": id, "success": true, "created": false})),
        )
            .into_response()
    }
}

async fn call(
    router: &axum::Router,
    method: &str,
    uri: &str,
    key: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn approved_upserts_apply_once_and_extraction_converges() {
    let Some(db) = common::fresh_db("upsert_apply").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let router = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let (s, _) = call(
        &router,
        "POST",
        "/v1/orgs",
        "op-secret",
        Some(json!({"name": "acme", "config_path": "/tmp/acme-upsert.yaml"})),
    )
    .await;
    assert!(s.is_success());
    let mut keys = HashMap::new();
    for role in ["admin", "steward", "viewer"] {
        let (_, k) = call(
            &router,
            "POST",
            "/v1/orgs/acme/api-keys",
            "op-secret",
            Some(json!({"role": role})),
        )
        .await;
        keys.insert(role, k["key"].as_str().unwrap().to_owned());
    }
    let org = pg::get_org(&client, "acme").await.unwrap().unwrap();
    let ds = pg::default_dataset_id(&client, &org.id)
        .await
        .unwrap()
        .unwrap();

    let sf: Shared = Arc::new(Mutex::new(Sf::default()));
    let mock = axum::Router::new()
        .route("/services/oauth2/token", post(token))
        .route(
            "/services/data/v62.0/sobjects/{object}/{field}/{key}",
            get(read).patch(upsert),
        )
        .with_state(Arc::clone(&sf));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    sf.lock().unwrap().base = base.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, mock).await;
    });
    let conn = pg::create_salesforce_connection(
        &client,
        &org.id,
        "prod",
        &base,
        "client_credentials",
        "cid",
        "sec",
        "v62.0",
    )
    .await
    .unwrap();
    er_entities::salesforce_delegated_grant::create::grant(
        &client,
        &er_ids::new_v7(),
        &org.id,
        &conn,
        "00Dorg",
        "005user",
        None,
        "r-good",
        "api refresh_token",
        false,
        "key:admin",
    )
    .await
    .unwrap();
    pg::record_provenance(
        &client,
        &pg::DeliveryProvenance {
            org_id: &org.id,
            dataset_id: &ds,
            run_id: "run-sf",
            delivery_id: "delivery-sf",
            source: "sfdc",
            connection_id: &conn,
            object: "Contact",
            delivery_sha256: "d",
            source_sha256: "s",
        },
    )
    .await
    .unwrap();

    // --- Policy validation -----------------------------------------------------------
    let policy_uri = "/v1/orgs/acme/datasets/default/writeback/upsert-policies";
    let good = json!({"connection_id": conn, "object": "Contact", "external_id_field": "DupeZero_Id__c",
        "field_mapping": {"email": "Email", "family_name": "LastName"}, "writeback_allowlist": ["Email", "LastName"],
        "sources": ["webforms"]});
    let (status, _) = call(
        &router,
        "PUT",
        policy_uri,
        &keys["steward"],
        Some(good.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for bad in [
        json!({"connection_id": conn, "object": "Opportunity", "external_id_field": "X__c", "field_mapping": {}, "writeback_allowlist": [], "sources": []}),
        json!({"connection_id": conn, "object": "Contact", "external_id_field": "X__c", "field_mapping": {"nope": "Email"}, "writeback_allowlist": [], "sources": []}),
        json!({"connection_id": conn, "object": "Contact", "external_id_field": "X__c", "field_mapping": {}, "writeback_allowlist": ["Id"], "sources": []}),
    ] {
        let (status, body) = call(&router, "PUT", policy_uri, &keys["admin"], Some(bad)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
    let (status, policy) = call(&router, "PUT", policy_uri, &keys["admin"], Some(good)).await;
    assert_eq!(status, StatusCode::OK, "{policy}");
    let policy_id = policy["id"].as_str().unwrap().to_owned();
    let (status, _) = call(
        &router,
        "POST",
        "/v1/orgs/acme/datasets/default/upsert-previews/apply",
        &keys["admin"],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "nothing approved");

    // --- Five proposals ----------------------------------------------------------------
    let mut ids = BTreeMap::new();
    for (key, email) in [
        ("ent-create", "c@x.test"),
        ("ent-update", "u@x.test"),
        ("ent-stale", "s@x.test"),
        ("ent-lost", "l@x.test"),
        ("ent-refused", "r@x.test"),
    ] {
        let mut built = er_server::preview_job::BuiltUpsert {
            origin_run_id: "run-1".into(),
            policy_id: policy_id.clone(),
            connection_id: conn.clone(),
            object: "Contact".into(),
            entity_id: key.into(),
            external_id_field: "DupeZero_Id__c".into(),
            external_id_value: key.into(),
            anchor_record_key: format!("webforms:{key}"),
            fields: BTreeMap::from([("Email".to_string(), json!(email))]),
            fingerprint: String::new(),
        };
        built.fingerprint = er_server::preview_job::upsert_fingerprint(&built);
        let row = er_entities::upsert_preview::create::publish(
            &client,
            &er_ids::new_v7(),
            &org.id,
            &ds,
            &conn,
            "Contact",
            key,
            "DupeZero_Id__c",
            key,
            &built.anchor_record_key,
            &json!(built.fields),
            &built.fingerprint,
            3,
            "run-1",
            &policy_id,
        )
        .await
        .unwrap()
        .remove(0);
        ids.insert(key, (row.id, row.fingerprint));
    }
    {
        let mut s = sf.lock().unwrap();
        // Existing records: one untouched since long before the proposal, one edited after it.
        s.by_key.insert(
            "ent-update".into(),
            (
                "003E00000000001".into(),
                "2020-01-01T00:00:00.000+0000".into(),
                BTreeMap::from([("Email".to_string(), json!("old@x.test"))]),
            ),
        );
        s.by_key.insert(
            "ent-stale".into(),
            (
                "003E00000000002".into(),
                "2099-01-01T00:00:00.000+0000".into(),
                BTreeMap::from([("Email".to_string(), json!("theirs@x.test"))]),
            ),
        );
        s.lose.push("ent-lost".into());
        s.refuse.push("ent-refused".into());
    }

    // --- Decisions --------------------------------------------------------------------
    let decide_uri = "/v1/orgs/acme/datasets/default/upsert-previews/decide";
    let items: Vec<Value> = ids
        .values()
        .map(|(id, fp)| json!({"id": id, "fingerprint": fp}))
        .collect();
    let (status, _) = call(
        &router,
        "POST",
        decide_uri,
        &keys["viewer"],
        Some(json!({"decision": "approve", "items": items.clone()})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let mut with_stale = items.clone();
    with_stale.push(json!({"id": ids["ent-create"].0, "fingerprint": "v1:wrong"}));
    let (status, decided) = call(
        &router,
        "POST",
        decide_uri,
        &keys["steward"],
        Some(json!({"decision": "approve", "items": with_stale})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{decided}");
    let statuses: Vec<u64> = decided["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["status"].as_u64().unwrap())
        .collect();
    assert_eq!(statuses, [200, 200, 200, 200, 200, 409], "{decided}");

    // --- Run 1 --------------------------------------------------------------------------
    let limits = Limits {
        backoff: std::time::Duration::from_millis(5),
        ..Limits::default()
    };
    let run1 = upsert_apply::run(&client, &org.id, &ds, "job-1", Egress::Unguarded, limits)
        .await
        .unwrap();
    let apply_of = |key: &'static str| {
        let (client, org, ds, id) = (&client, org.id.clone(), ds.clone(), ids[key].0.clone());
        async move {
            er_entities::upsert_apply::read::by_preview(client, &org, &ds, &id)
                .await
                .unwrap()
                .remove(0)
        }
    };
    let created = apply_of("ent-create").await;
    assert_eq!(
        (created.status.as_str(), created.preimage.as_str()),
        ("created", "null")
    );
    assert!(created.sf_record_id.as_deref().unwrap().starts_with("003N"));
    let updated = apply_of("ent-update").await;
    assert_eq!(updated.status, "updated");
    assert!(
        updated.preimage.contains("old@x.test"),
        "{}",
        updated.preimage
    );
    assert_eq!(apply_of("ent-stale").await.status, "skipped_stale");
    let lost = apply_of("ent-lost").await;
    assert_eq!(
        (lost.status.as_str(), lost.call_state.as_str()),
        ("applying", "in_flight")
    );
    let refused = apply_of("ent-refused").await;
    assert_eq!(refused.status, "failed");
    assert!(
        refused
            .detail
            .unwrap()
            .contains("FIELD_INTEGRITY_EXCEPTION")
    );
    assert_eq!(
        run1.assertions_staged, 1,
        "the created record converges: {run1:?}"
    );
    {
        let s = sf.lock().unwrap();
        assert!(
            !s.writes.contains(&"ent-stale".to_string()),
            "a stale record is never written"
        );
        assert_eq!(s.by_key["ent-stale"].2["Email"], json!("theirs@x.test"));
    }
    let staged = pg::pending_actions(&client, &ds).await.unwrap();
    let assertion = staged
        .iter()
        .find(|a| a.action["kind"] == "always")
        .expect("the always-match assertion is staged");
    assert_eq!(assertion.action["a"], "webforms:ent-create");
    assert_eq!(
        assertion.action["b"],
        format!("sfdc:{}", created.sf_record_id.clone().unwrap())
    );

    // --- Run 2: the lost upsert is verified, not written again -----------------------
    let writes = sf
        .lock()
        .unwrap()
        .writes
        .iter()
        .filter(|k| *k == "ent-lost")
        .count();
    upsert_apply::run(&client, &org.id, &ds, "job-2", Egress::Unguarded, limits)
        .await
        .unwrap();
    let lost = apply_of("ent-lost").await;
    assert_eq!(lost.status, "created", "{:?}", lost.detail);
    assert!(lost.detail.unwrap().contains("verified"));
    assert_eq!(
        sf.lock()
            .unwrap()
            .writes
            .iter()
            .filter(|k| *k == "ent-lost")
            .count(),
        writes,
        "no second write"
    );
    assert_eq!(
        sf.lock().unwrap().by_key.len(),
        4,
        "no record was created twice"
    );

    // --- The list shows outcomes --------------------------------------------------------
    let (_, page) = call(
        &router,
        "GET",
        "/v1/orgs/acme/datasets/default/upsert-previews",
        &keys["viewer"],
        None,
    )
    .await;
    let shown: Vec<&str> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["apply_status"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(shown.len(), 5, "{page}");
    assert!(
        shown.contains(&"skipped_stale") && shown.contains(&"updated"),
        "{page}"
    );
}
