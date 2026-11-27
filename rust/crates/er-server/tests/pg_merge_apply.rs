//! #995: applying approved merge previews to a mock Salesforce, journaled.
//!
//! Eight approved groups, each driving one path: a clean apply; a stale
//! member (skipped, resync queued); an external merge (applied externally,
//! no call made); a missing victim (failed — absence is no evidence); a 429
//! retried; a merge whose response is lost (left in flight, resolved by a
//! re-read on the next run, never replayed); a rejected merge (failed, the
//! batch continues); and an exhausted API budget (the job stops, the group
//! resumes next run). Plus the route's role and nothing-to-do answers.
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Path as AxPath, State as AxState};
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, patch, post};
use er_server::merge_apply::{self, Limits};
use er_server::urlguard::Egress;
use er_server::writeback::{
    MemberSnapshot, MergeObject, MergeProposal, PersistedProposalV1, ProposalInput, ProposalScope,
    ProviderScope,
};
use er_server::{AppState, app, pg};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::common;

const STAMP: &str = "2026-11-27T00:00:00.000+0000";

#[derive(Clone)]
struct Rec {
    stamp: String,
    deleted: bool,
    master: Option<String>,
}

#[derive(Default)]
struct Sf {
    records: BTreeMap<String, Rec>,
    /// Master id → how its next PATCH answers once: "429".
    patch_once: HashMap<String, &'static str>,
    /// Master id → how its merge answers: "lost" (performed, then 500), "reject".
    merge_mode: HashMap<String, &'static str>,
    /// While set, a query naming this id prefix answers REQUEST_LIMIT_EXCEEDED.
    over_budget_for: Option<String>,
    base: String,
    calls: Vec<String>,
    stamp_seq: u32,
}

impl Sf {
    fn bump(&mut self, id: &str) {
        self.stamp_seq += 1;
        let s = format!("2026-11-27T01:00:{:02}.000+0000", self.stamp_seq);
        if let Some(r) = self.records.get_mut(id) {
            r.stamp = s;
        }
    }
}

type Shared = Arc<Mutex<Sf>>;

async fn token(AxState(sf): AxState<Shared>) -> axum::Json<Value> {
    let base = sf.lock().unwrap().base.clone();
    axum::Json(json!({"access_token": "at", "instance_url": base, "id": ""}))
}

async fn query_all(
    AxState(sf): AxState<Shared>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> axum::response::Response {
    let mut sf = sf.lock().unwrap();
    let soql = q.get("q").cloned().unwrap_or_default();
    if sf
        .over_budget_for
        .as_ref()
        .is_some_and(|p| soql.contains(p.as_str()))
    {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!([{"errorCode": "REQUEST_LIMIT_EXCEEDED"}])),
        )
            .into_response();
    }
    sf.calls.push("query".into());
    let ids: Vec<String> = soql
        .split('\'')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect();
    let records: Vec<Value> = ids
        .iter()
        .filter_map(|id| sf.records.get(id).map(|r| (id, r)))
        .map(|(id, r)| json!({"Id": id, "SystemModstamp": r.stamp, "IsDeleted": r.deleted, "MasterRecordId": r.master}))
        .collect();
    axum::Json(json!({"records": records})).into_response()
}

async fn update(
    AxState(sf): AxState<Shared>,
    AxPath((_object, id)): AxPath<(String, String)>,
) -> axum::response::Response {
    let mut sf = sf.lock().unwrap();
    sf.calls.push(format!("patch {id}"));
    if sf.patch_once.remove(&id) == Some("429") {
        let mut h = HeaderMap::new();
        h.insert(header::RETRY_AFTER, "0".parse().unwrap());
        return (StatusCode::TOO_MANY_REQUESTS, h, "[]").into_response();
    }
    sf.bump(&id);
    StatusCode::NO_CONTENT.into_response()
}

async fn soap(AxState(sf): AxState<Shared>, body: String) -> axum::response::Response {
    let mut sf = sf.lock().unwrap();
    let master = body
        .split("<urn1:Id>")
        .nth(1)
        .and_then(|s| s.split('<').next())
        .unwrap_or_default()
        .to_string();
    let victims: Vec<String> = body
        .split("<urn:recordToMergeIds>")
        .skip(1)
        .filter_map(|s| s.split('<').next())
        .map(str::to_string)
        .collect();
    sf.calls.push(format!("merge {master}"));
    let mode = sf.merge_mode.get(&master).copied();
    if mode == Some("reject") {
        return "<soapenv:Envelope><result><success>false</success><errors><statusCode>ENTITY_IS_LOCKED</statusCode></errors></result></soapenv:Envelope>".into_response();
    }
    for v in &victims {
        if let Some(r) = sf.records.get_mut(v) {
            r.deleted = true;
            r.master = Some(master.clone());
        }
    }
    sf.bump(&master);
    if mode == Some("lost") {
        sf.merge_mode.remove(&master);
        return (StatusCode::INTERNAL_SERVER_ERROR, "gateway").into_response();
    }
    "<soapenv:Envelope><result><success>true</success></result></soapenv:Envelope>".into_response()
}

fn sfid(group: char, n: u32) -> String {
    format!("003{group}{n:011}")
}

fn input(org: &str, ds: &str, conn: &str, policy: &str, group: char) -> ProposalInput {
    let provider = ProviderScope {
        org_id: org.into(),
        dataset_id: ds.into(),
        connection_id: conn.into(),
        object: MergeObject::Contact,
    };
    let member = |n: u32, email: &str| MemberSnapshot {
        provider: provider.clone(),
        record_key: format!("crm:{group}{n}"),
        external_id: sfid(group, n),
        system_modstamp: STAMP.into(),
        fields: BTreeMap::from([("Email".to_string(), json!(email))]),
    };
    ProposalInput {
        scope: ProposalScope {
            provider: provider.clone(),
            entity_id: format!("entity-{group}"),
            lake_snapshot: 3,
            policy_revision: policy.into(),
            election_policy: er_golden::DEFAULT_MASTER_ELECTION_POLICY.into(),
        },
        master_record_key: format!("crm:{group}1"),
        members: vec![member(1, "old@x.test"), member(2, "new@x.test")],
        golden_fields: BTreeMap::from([("email".to_string(), json!("new@x.test"))]),
        field_mapping: BTreeMap::from([("email".to_string(), "Email".to_string())]),
        writeback_allowlist: BTreeSet::from(["Email".to_string()]),
    }
}

async fn call(router: &axum::Router, uri: &str, key: &str) -> (StatusCode, Value) {
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .body(Body::empty())
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
async fn approved_merges_apply_through_the_journal_and_fail_closed() {
    let Some(db) = common::fresh_db("merge_apply").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let router = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let create = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/orgs")
                .header(header::AUTHORIZATION, "Bearer op-secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"name": "acme", "config_path": "/tmp/acme-apply.yaml"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(create.status().is_success(), "{}", create.status());
    let mut keys = BTreeMap::new();
    for role in ["admin", "viewer"] {
        let resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/orgs/acme/api-keys")
                    .header(header::AUTHORIZATION, "Bearer op-secret")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({"role": role}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        keys.insert(role, v["key"].as_str().unwrap().to_owned());
    }
    let org = pg::get_org(&client, "acme").await.unwrap().unwrap();
    let ds = pg::default_dataset_id(&client, &org.id)
        .await
        .unwrap()
        .unwrap();
    let apply_uri = "/v1/orgs/acme/datasets/default/merge-previews/apply";
    let (status, _) = call(&router, apply_uri, &keys["admin"]).await;
    assert_eq!(status, StatusCode::CONFLICT, "nothing approved yet");

    // --- The mock Salesforce and the delegated grant ----------------------------
    let sf: Shared = Arc::new(Mutex::new(Sf::default()));
    let mock = axum::Router::new()
        .route("/services/oauth2/token", post(token))
        .route("/services/data/v62.0/queryAll", get(query_all))
        .route("/services/data/v62.0/sobjects/{object}/{id}", patch(update))
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
    pg::record_provenance(
        &client,
        &pg::DeliveryProvenance {
            org_id: &org.id,
            dataset_id: &ds,
            run_id: "run-1",
            delivery_id: "delivery-1",
            source: "crm",
            connection_id: &conn,
            object: "Contact",
            delivery_sha256: "d",
            source_sha256: "s",
        },
    )
    .await
    .unwrap();

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/v1/orgs/acme/datasets/default/writeback/policies")
                .header(header::AUTHORIZATION, format!("Bearer {}", keys["admin"]))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"connection_id": conn, "object": "Contact",
                           "writeback_allowlist": ["Email"], "field_mapping": {"email": "email_address"}})
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let policy_id = serde_json::from_slice::<Value>(&bytes).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // --- Eight approved groups --------------------------------------------------
    let groups = ['A', 'B', 'C', 'D', 'E', 'F', 'G', 'H'];
    let mut preview_of = BTreeMap::new();
    {
        let mut s = sf.lock().unwrap();
        for g in groups {
            for n in [1, 2] {
                s.records.insert(
                    sfid(g, n),
                    Rec {
                        stamp: STAMP.into(),
                        deleted: false,
                        master: None,
                    },
                );
            }
        }
        s.records.get_mut(&sfid('B', 2)).unwrap().stamp = "2026-11-27T05:00:00.000+0000".into();
        let c2 = s.records.get_mut(&sfid('C', 2)).unwrap();
        c2.deleted = true;
        c2.master = Some(sfid('C', 1));
        s.records.remove(&sfid('D', 2));
        s.patch_once.insert(sfid('E', 1), "429");
        s.merge_mode.insert(sfid('F', 1), "lost");
        s.merge_mode.insert(sfid('G', 1), "reject");
        s.over_budget_for = Some("003H".into());
    }
    for g in groups {
        let input = input(&org.id, &ds, &conn, &policy_id, g);
        let proposal = MergeProposal::build(input.clone()).unwrap();
        let persisted = serde_json::to_value(PersistedProposalV1::new(input)).unwrap();
        let row = pg::publish_preview(
            &client,
            &pg::PreviewToPublish {
                org_id: &org.id,
                dataset_id: &ds,
                origin_run_id: "run-1",
                policy_id: &policy_id,
                proposal: &proposal,
                persisted: &persisted,
                actor: "job:test",
                score: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
        let decided = pg::decide_preview(
            &mut client,
            &org.id,
            &ds,
            &row.id,
            &row.fingerprint,
            "approved",
            "key:steward",
            None,
        )
        .await
        .unwrap();
        assert!(matches!(decided, pg::Decision::Decided(_)));
        preview_of.insert(g, row.id);
    }

    // --- The route ---------------------------------------------------------------
    let (status, _) = call(&router, apply_uri, &keys["viewer"]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, queued) = call(&router, apply_uri, &keys["admin"]).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");
    let job_id = queued["job_id"].as_str().unwrap().to_owned();

    let limits = Limits {
        backoff: std::time::Duration::from_millis(5),
        ..Limits::default()
    };
    let status_of = |g: char| {
        let (client, org, ds, id) = (&client, org.id.clone(), ds.clone(), preview_of[&g].clone());
        async move {
            er_entities::merge_apply::read::by_preview(client, &org, &ds, &id)
                .await
                .unwrap()
                .into_iter()
                .next()
                .map(|a| (a.status, a.detail.unwrap_or_default(), a.preimage))
        }
    };

    // --- Run 1: every path; H's query finds the API budget exhausted --------
    let run1 = merge_apply::run(&client, &org.id, &ds, &job_id, Egress::Unguarded, limits)
        .await
        .unwrap();
    assert_eq!(status_of('A').await.unwrap().0, "applied");
    let (b, detail, _) = status_of('B').await.unwrap();
    assert_eq!(
        (b.as_str(), detail.contains("crm:B2")),
        ("skipped_stale", true),
        "{detail}"
    );
    assert_eq!(status_of('C').await.unwrap().0, "applied_externally");
    let (d, detail, _) = status_of('D').await.unwrap();
    assert_eq!(d, "failed", "a missing victim is no evidence: {detail}");
    assert_eq!(
        status_of('E').await.unwrap().0,
        "applied",
        "the 429 was retried"
    );
    assert_eq!(
        status_of('F').await.unwrap().0,
        "applying",
        "a lost response stays in flight"
    );
    let (g, detail, _) = status_of('G').await.unwrap();
    assert_eq!(
        (g.as_str(), detail.contains("ENTITY_IS_LOCKED")),
        ("failed", true),
        "{detail}"
    );
    assert_eq!(
        status_of('H').await.unwrap().0,
        "applying",
        "begun, then the budget stopped the job"
    );
    assert_eq!(run1.stopped.as_deref(), Some("budget"), "{run1:?}");
    assert_eq!(run1.resyncs_queued, 1, "{run1:?}");
    let (_, _, preimage) = status_of('A').await.unwrap();
    assert!(
        preimage.contains("old@x.test") && preimage.contains("live_modstamps"),
        "{preimage}"
    );
    {
        let s = sf.lock().unwrap();
        assert!(
            !s.calls
                .iter()
                .any(|c| c.contains("003C") || c.contains("003D") || c == "patch 003B00000000001"),
            "no mutation for C, D or the stale B: {:?}",
            s.calls
        );
        assert!(
            s.records[&sfid('A', 2)].deleted
                && s.records[&sfid('A', 2)].master.as_deref() == Some(sfid('A', 1).as_str())
        );
    }

    // --- Run 2: the in-flight merge is verified by re-reading, not replayed ------
    sf.lock().unwrap().over_budget_for = None;
    let merges_before = sf
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|c| c.as_str() == "merge 003F00000000001")
        .count();
    let run2 = merge_apply::run(&client, &org.id, &ds, &job_id, Egress::Unguarded, limits)
        .await
        .unwrap();
    assert_eq!(status_of('F').await.unwrap().0, "applied", "{run2:?}");
    assert_eq!(status_of('H').await.unwrap().0, "applied", "{run2:?}");
    let merges_after = sf
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|c| c.as_str() == "merge 003F00000000001")
        .count();
    assert_eq!(
        merges_before, merges_after,
        "the lost merge was not replayed"
    );
    let f_apply =
        er_entities::merge_apply::read::by_preview(&client, &org.id, &ds, &preview_of[&'F'])
            .await
            .unwrap()
            .remove(0);
    let ops = er_entities::merge_apply_operation::read::by_apply(&client, &org.id, &f_apply.id)
        .await
        .unwrap();
    assert!(
        ops.iter().any(|o| o.kind == "merge"
            && o.status == "done"
            && o.outcome.as_deref() == Some("verified")),
        "{:?}",
        ops.iter()
            .map(|o| (&o.kind, &o.status, &o.outcome))
            .collect::<Vec<_>>()
    );

    // The history a reviewer reads.
    let history = er_entities::merge_preview_journal::read::by_preview(
        &client,
        &org.id,
        &ds,
        &preview_of[&'A'],
    )
    .await
    .unwrap();
    let path: Vec<&str> = history.iter().map(|h| h.to_status.as_str()).collect();
    assert_eq!(
        path,
        ["planned", "approved", "applying", "applied"],
        "{path:?}"
    );

    // Run 3: nothing left; a terminal apply is never revisited.
    let calls = sf.lock().unwrap().calls.len();
    let run3 = merge_apply::run(&client, &org.id, &ds, &job_id, Egress::Unguarded, limits)
        .await
        .unwrap();
    assert_eq!(
        (run3.applied, run3.failed, run3.deferred),
        (0, 0, 0),
        "{run3:?}"
    );
    assert_eq!(sf.lock().unwrap().calls.len(), calls);
}
