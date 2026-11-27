//! #520: converting duplicate Leads into their existing Contacts against a
//! mock Salesforce.
//!
//! Six approved conversions, one per path: a clean conversion (Lead
//! converted into the Contact's Account, then a FULL Lead extract and an
//! incremental Contact extract queued so the Lead leaves the corpus); a Lead
//! already converted into the same Contact outside DupeZero (recorded,
//! nothing sent); one converted into a different Contact (failed — not
//! reversible, never retried); a stale Lead (skipped); a lost response
//! (left in flight, then verified by re-reading the Lead — no second call);
//! a Contact without an Account (failed). Plus decide/apply route rules.
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State as AxState;
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use er_server::merge_apply::Limits;
use er_server::urlguard::Egress;
use er_server::{AppState, app, lead_convert, pg};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::common;

const STAMP: &str = "2026-11-27T00:00:00.000+0000";

#[derive(Default)]
struct Sf {
    /// id → record fields (Lead or Contact)
    records: BTreeMap<String, serde_json::Map<String, Value>>,
    /// lead ids whose conversion lands but answers 500
    lose: Vec<String>,
    converts: Vec<String>,
    base: String,
}

type Shared = Arc<Mutex<Sf>>;

async fn token(AxState(sf): AxState<Shared>) -> axum::Json<Value> {
    let base = sf.lock().unwrap().base.clone();
    axum::Json(json!({"access_token": "at", "instance_url": base, "id": ""}))
}

async fn query(
    AxState(sf): AxState<Shared>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> axum::Json<Value> {
    let sf = sf.lock().unwrap();
    let soql = &q["q"];
    if soql.contains("FROM LeadStatus") {
        return axum::Json(json!({"records": [{"MasterLabel": "Closed - Converted"}]}));
    }
    let id = soql.split('\'').nth(1).unwrap_or_default();
    let records: Vec<Value> = sf
        .records
        .get(id)
        .map(|r| Value::Object(r.clone()))
        .into_iter()
        .collect();
    axum::Json(json!({"records": records}))
}

async fn soap(AxState(sf): AxState<Shared>, body: String) -> axum::response::Response {
    let mut sf = sf.lock().unwrap();
    let take = |tag: &str| {
        body.split(&format!("<urn:{tag}>"))
            .nth(1)
            .and_then(|s| s.split('<').next())
            .unwrap_or_default()
            .to_string()
    };
    let (lead, contact) = (take("leadId"), take("contactId"));
    sf.converts.push(lead.clone());
    if let Some(r) = sf.records.get_mut(&lead) {
        r.insert("IsConverted".into(), json!(true));
        r.insert("ConvertedContactId".into(), json!(contact));
    }
    if let Some(i) = sf.lose.iter().position(|l| l == &lead) {
        sf.lose.remove(i);
        return (StatusCode::INTERNAL_SERVER_ERROR, "gateway").into_response();
    }
    "<soapenv:Envelope><result><success>true</success></result></soapenv:Envelope>".into_response()
}

fn lead(id: &str, stamp: &str, converted_into: Option<&str>) -> serde_json::Map<String, Value> {
    json!({"Id": id, "SystemModstamp": stamp, "IsDeleted": false,
           "IsConverted": converted_into.is_some(), "ConvertedContactId": converted_into})
    .as_object()
    .unwrap()
    .clone()
}

fn contact(id: &str, account: Option<&str>) -> serde_json::Map<String, Value> {
    json!({"Id": id, "SystemModstamp": STAMP, "IsDeleted": false, "AccountId": account})
        .as_object()
        .unwrap()
        .clone()
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
async fn approved_lead_conversions_apply_once_and_never_blind() {
    let Some(db) = common::fresh_db("lead_convert").await else {
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
        Some(json!({"name": "acme", "config_path": "/tmp/acme-convert.yaml"})),
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
        .route("/services/data/v62.0/queryAll", get(query))
        .route("/services/data/v62.0/query", get(query))
        .route("/services/Soap/u/62.0", post(soap))
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
    for (source, object) in [("sf_leads", "Lead"), ("sf_contacts", "Contact")] {
        pg::record_provenance(
            &client,
            &pg::DeliveryProvenance {
                org_id: &org.id,
                dataset_id: &ds,
                run_id: &format!("run-{object}"),
                delivery_id: &format!("delivery-{object}"),
                source,
                connection_id: &conn,
                object,
                delivery_sha256: "d",
                source_sha256: "s",
            },
        )
        .await
        .unwrap();
    }
    let mut policy = BTreeMap::new();
    for object in ["Lead", "Contact"] {
        let (status, p) = call(
            &router,
            "PUT",
            "/v1/orgs/acme/datasets/default/writeback/policies",
            &keys["admin"],
            Some(json!({"connection_id": conn, "object": object, "writeback_allowlist": [], "field_mapping": {}})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{p}");
        policy.insert(object, p["id"].as_str().unwrap().to_owned());
    }
    let (status, _) = call(
        &router,
        "POST",
        "/v1/orgs/acme/datasets/default/lead-conversions/apply",
        &keys["admin"],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "nothing approved");

    // --- Six proposals ------------------------------------------------------------------
    let cases = [
        ("A", "clean"),
        ("B", "external"),
        ("C", "foreign"),
        ("D", "stale"),
        ("E", "lost"),
        ("F", "no_account"),
    ];
    let mut ids = BTreeMap::new();
    {
        let mut s = sf.lock().unwrap();
        for (g, case) in cases {
            let (l, c) = (format!("00Q{g}00000000001"), format!("003{g}00000000001"));
            let into = match case {
                "external" => Some(c.clone()),
                "foreign" => Some("003Z00000000099".to_string()),
                _ => None,
            };
            let stamp = if case == "stale" {
                "2026-11-27T09:00:00.000+0000"
            } else {
                STAMP
            };
            s.records
                .insert(l.clone(), lead(&l, stamp, into.as_deref()));
            let account = if case == "no_account" {
                None
            } else {
                Some("001A00000000001")
            };
            s.records.insert(c.clone(), contact(&c, account));
        }
        s.lose.push("00QE00000000001".into());
    }
    for (g, _) in cases {
        let mut built = er_server::preview_job::BuiltConversion {
            origin_run_id: "run-1".into(),
            connection_id: conn.clone(),
            entity_id: format!("ent-{g}"),
            lead_external_id: format!("00Q{g}00000000001"),
            lead_record_key: format!("sf_leads:00Q{g}00000000001"),
            lead_modstamp: STAMP.into(),
            contact_external_id: format!("003{g}00000000001"),
            contact_record_key: format!("sf_contacts:003{g}00000000001"),
            contact_modstamp: STAMP.into(),
            lead_policy_id: policy["Lead"].clone(),
            contact_policy_id: policy["Contact"].clone(),
            fingerprint: String::new(),
        };
        built.fingerprint = er_server::preview_job::conversion_fingerprint(&built);
        let row = er_entities::lead_conversion::create::publish(
            &client,
            &er_ids::new_v7(),
            &org.id,
            &ds,
            &conn,
            &built.entity_id,
            &built.lead_external_id,
            &built.lead_record_key,
            &built.lead_modstamp,
            &built.contact_external_id,
            &built.contact_record_key,
            &built.contact_modstamp,
            &built.fingerprint,
            3,
            "run-1",
            &built.lead_policy_id,
            &built.contact_policy_id,
        )
        .await
        .unwrap()
        .remove(0);
        ids.insert(g, (row.id, row.fingerprint));
    }
    let decide_uri = "/v1/orgs/acme/datasets/default/lead-conversions/decide";
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
    let (status, decided) = call(
        &router,
        "POST",
        decide_uri,
        &keys["steward"],
        Some(json!({"decision": "approve", "items": items})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{decided}");
    assert!(
        decided["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["status"] == 200),
        "{decided}"
    );

    // --- Run 1 ----------------------------------------------------------------------------
    let limits = Limits {
        backoff: std::time::Duration::from_millis(5),
        ..Limits::default()
    };
    let run1 = lead_convert::run(&client, &org.id, &ds, "job-1", Egress::Unguarded, limits)
        .await
        .unwrap();
    let state = |g: &'static str| {
        let (client, org, ds, id) = (&client, org.id.clone(), ds.clone(), ids[g].0.clone());
        async move {
            er_entities::lead_conversion_apply::read::by_conversion(client, &org, &ds, &id)
                .await
                .unwrap()
                .remove(0)
        }
    };
    let a = state("A").await;
    assert_eq!(
        (a.status.as_str(), a.account_id.as_deref()),
        ("converted", Some("001A00000000001"))
    );
    assert_eq!(state("B").await.status, "converted_externally");
    let c = state("C").await;
    assert_eq!(c.status, "failed");
    assert!(c.detail.unwrap().contains("not reversible"));
    assert_eq!(state("D").await.status, "skipped_stale");
    let e = state("E").await;
    assert_eq!(
        (e.status.as_str(), e.call_state.as_str()),
        ("applying", "in_flight")
    );
    assert_eq!(state("F").await.status, "failed");
    {
        let s = sf.lock().unwrap();
        for never in [
            "00QB00000000001",
            "00QC00000000001",
            "00QD00000000001",
            "00QF00000000001",
        ] {
            assert!(
                !s.converts.contains(&never.to_string()),
                "{never} was not sent: {:?}",
                s.converts
            );
        }
    }
    // A converted Lead leaves the corpus: a FULL Lead extract (which omits
    // converted Leads, so ingest tombstones it) and a Contact extract.
    assert_eq!(
        run1.extracts_queued, 4,
        "two per conversion (A and B): {run1:?}"
    );

    // --- Run 2: the lost call is verified by re-reading the Lead ----------------------
    let before = sf.lock().unwrap().converts.len();
    lead_convert::run(&client, &org.id, &ds, "job-2", Egress::Unguarded, limits)
        .await
        .unwrap();
    let e = state("E").await;
    assert_eq!(e.status, "converted", "{:?}", e.detail);
    assert!(e.detail.unwrap().contains("verified"));
    assert_eq!(
        sf.lock().unwrap().converts.len(),
        before,
        "no second convertLead"
    );

    let (_, page) = call(
        &router,
        "GET",
        "/v1/orgs/acme/datasets/default/lead-conversions",
        &keys["viewer"],
        None,
    )
    .await;
    assert_eq!(page["items"].as_array().unwrap().len(), 6, "{page}");
}
