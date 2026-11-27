//! #996: undoing applied merges against a mock Salesforce.
//!
//! Three groups are applied (#995), then undone:
//! - A: the victim is in the Recycle Bin and the master still holds what the
//!   apply wrote → undeleted, master reverted, `restored`;
//! - B: the bin was emptied and someone edited the master's Email since →
//!   the victim is recreated (old → new id recorded) and the Email is a
//!   conflict, not overwritten → `partial`;
//! - C: the undelete's response is lost → left in flight; the next run sees
//!   the victim back and records it `verified` without a second call.
//!
//! Plus: never-match assertions staged, a repeated undo returns the same
//! undo and queues nothing, role and cross-tenant access.
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Path as AxPath, State as AxState};
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, patch, post};
use er_server::merge_apply::{self, Limits};
use er_server::urlguard::Egress;
use er_server::writeback::{
    MemberSnapshot, MergeObject, MergeProposal, PersistedProposalV1, ProposalInput, ProposalScope,
    ProviderScope,
};
use er_server::{AppState, app, merge_restore, pg};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::common;

const STAMP: &str = "2026-11-27T00:00:00.000+0000";

#[derive(Clone)]
struct Rec {
    stamp: String,
    deleted: bool,
    master: Option<String>,
    fields: BTreeMap<String, Value>,
}

#[derive(Default)]
struct Sf {
    records: BTreeMap<String, Rec>,
    /// Ids whose next undelete is performed but answered 500.
    lose_undelete: BTreeSet<String>,
    calls: Vec<String>,
    created: u32,
    base: String,
}

type Shared = Arc<Mutex<Sf>>;

async fn token(AxState(sf): AxState<Shared>) -> axum::Json<Value> {
    let base = sf.lock().unwrap().base.clone();
    axum::Json(json!({"access_token": "at", "instance_url": base, "id": ""}))
}

fn quoted_ids(soql: &str) -> Vec<String> {
    soql.split('\'')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

async fn query_all(
    AxState(sf): AxState<Shared>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> axum::Json<Value> {
    let sf = sf.lock().unwrap();
    let records: Vec<Value> = quoted_ids(&q["q"])
        .iter()
        .filter_map(|id| sf.records.get(id).map(|r| (id, r)))
        .map(|(id, r)| json!({"Id": id, "SystemModstamp": r.stamp, "IsDeleted": r.deleted, "MasterRecordId": r.master}))
        .collect();
    axum::Json(json!({"records": records}))
}

async fn query(
    AxState(sf): AxState<Shared>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> axum::Json<Value> {
    let sf = sf.lock().unwrap();
    let soql = &q["q"];
    let fields: Vec<&str> = soql["SELECT ".len()..soql.find(" FROM").unwrap()]
        .split(", ")
        .collect();
    let records: Vec<Value> = quoted_ids(soql)
        .iter()
        .filter_map(|id| sf.records.get(id))
        .filter(|r| !r.deleted)
        .map(|r| {
            Value::Object(
                fields
                    .iter()
                    .map(|f| {
                        (
                            (*f).to_string(),
                            r.fields.get(*f).cloned().unwrap_or(Value::Null),
                        )
                    })
                    .collect(),
            )
        })
        .collect();
    axum::Json(json!({"records": records}))
}

async fn update(
    AxState(sf): AxState<Shared>,
    AxPath((_object, id)): AxPath<(String, String)>,
    axum::Json(body): axum::Json<BTreeMap<String, Value>>,
) -> StatusCode {
    let mut sf = sf.lock().unwrap();
    sf.calls.push(format!("patch {id}"));
    let n = sf.calls.len();
    if let Some(r) = sf.records.get_mut(&id) {
        r.fields.extend(body);
        r.stamp = format!("{STAMP}-p{n}");
    }
    StatusCode::NO_CONTENT
}

async fn create(
    AxState(sf): AxState<Shared>,
    axum::Json(body): axum::Json<BTreeMap<String, Value>>,
) -> (StatusCode, axum::Json<Value>) {
    let mut sf = sf.lock().unwrap();
    sf.created += 1;
    let id = format!("003Z{:011}", sf.created);
    sf.calls.push(format!("create {id}"));
    sf.records.insert(
        id.clone(),
        Rec {
            stamp: STAMP.into(),
            deleted: false,
            master: None,
            fields: body,
        },
    );
    (
        StatusCode::CREATED,
        axum::Json(json!({"id": id, "success": true})),
    )
}

async fn soap(AxState(sf): AxState<Shared>, body: String) -> axum::response::Response {
    let mut sf = sf.lock().unwrap();
    if body.contains("<urn:undelete>") {
        let ids: Vec<String> = body
            .split("<urn:ids>")
            .skip(1)
            .filter_map(|s| s.split('<').next())
            .map(str::to_string)
            .collect();
        let mut results = String::new();
        let mut lost = false;
        for id in &ids {
            sf.calls.push(format!("undelete {id}"));
            lost |= sf.lose_undelete.remove(id);
            match sf.records.get_mut(id) {
                Some(r) if r.deleted => {
                    r.deleted = false;
                    r.master = None;
                    results.push_str("<result><success>true</success></result>");
                }
                _ => results.push_str("<result><success>false</success><errors><statusCode>UNDELETE_FAILED</statusCode></errors></result>"),
            }
        }
        if lost {
            return (StatusCode::INTERNAL_SERVER_ERROR, "gateway").into_response();
        }
        return format!("<soapenv:Envelope>{results}</soapenv:Envelope>").into_response();
    }
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
    for v in &victims {
        if let Some(r) = sf.records.get_mut(v) {
            r.deleted = true;
            r.master = Some(master.clone());
        }
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
        fields: BTreeMap::from([
            ("Email".to_string(), json!(email)),
            ("LastName".to_string(), json!(format!("L{group}{n}"))),
        ]),
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
async fn applied_merges_undo_from_the_journal_without_overwriting_later_edits() {
    let Some(db) = common::fresh_db("merge_restore").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let router = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let mut keys = BTreeMap::new();
    for org in ["acme", "globex"] {
        let (s, _) = call(
            &router,
            "POST",
            "/v1/orgs",
            "op-secret",
            Some(json!({"name": org, "config_path": format!("/tmp/{org}-restore.yaml")})),
        )
        .await;
        assert!(s.is_success(), "{s}");
        for role in ["admin", "viewer"] {
            let (_, k) = call(
                &router,
                "POST",
                &format!("/v1/orgs/{org}/api-keys"),
                "op-secret",
                Some(json!({"role": role})),
            )
            .await;
            keys.insert(
                format!("{org}:{role}"),
                k["key"].as_str().unwrap().to_owned(),
            );
        }
    }
    let org = pg::get_org(&client, "acme").await.unwrap().unwrap();
    let ds = pg::default_dataset_id(&client, &org.id)
        .await
        .unwrap()
        .unwrap();

    let sf: Shared = Arc::new(Mutex::new(Sf::default()));
    let mock = axum::Router::new()
        .route("/services/oauth2/token", post(token))
        .route("/services/data/v62.0/queryAll", get(query_all))
        .route("/services/data/v62.0/query", get(query))
        .route("/services/data/v62.0/sobjects/{object}", post(create))
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
    let (status, policy) = call(
        &router,
        "PUT",
        "/v1/orgs/acme/datasets/default/writeback/policies",
        &keys["acme:admin"],
        Some(json!({"connection_id": conn, "object": "Contact", "writeback_allowlist": ["Email"], "field_mapping": {"email": "email_address"}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{policy}");
    let policy_id = policy["id"].as_str().unwrap().to_owned();

    // --- Apply three groups (#995) ------------------------------------------------
    let groups = ['A', 'B', 'C'];
    let mut preview_of = BTreeMap::new();
    for g in groups {
        let input = input(&org.id, &ds, &conn, &policy_id, g);
        {
            let mut s = sf.lock().unwrap();
            for m in &input.members {
                s.records.insert(
                    m.external_id.clone(),
                    Rec {
                        stamp: STAMP.into(),
                        deleted: false,
                        master: None,
                        fields: m.fields.clone(),
                    },
                );
            }
        }
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
        pg::decide_preview(
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
        preview_of.insert(g, row.id);
    }
    let base_uri = "/v1/orgs/acme/datasets/default/merge-previews";
    let (status, _) = call(
        &router,
        "POST",
        &format!("{base_uri}/{}/restore", preview_of[&'A']),
        &keys["acme:admin"],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "nothing applied yet");
    let limits = Limits {
        backoff: std::time::Duration::from_millis(5),
        ..Limits::default()
    };
    let applied = merge_apply::run(
        &client,
        &org.id,
        &ds,
        "job-apply",
        Egress::Unguarded,
        limits,
    )
    .await
    .unwrap();
    assert_eq!(applied.applied, 3, "{applied:?}");

    // After the apply: B's bin is emptied and someone edits B's master.
    {
        let mut s = sf.lock().unwrap();
        s.records.remove(&sfid('B', 2));
        s.records
            .get_mut(&sfid('B', 1))
            .unwrap()
            .fields
            .insert("Email".into(), json!("edited@x.test"));
        s.lose_undelete.insert(sfid('C', 2));
    }

    // --- Access -----------------------------------------------------------------------
    let a_uri = format!("{base_uri}/{}/restore", preview_of[&'A']);
    let (status, _) = call(&router, "POST", &a_uri, &keys["acme:viewer"], None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &router,
        "POST",
        &format!(
            "/v1/orgs/globex/datasets/default/merge-previews/{}/restore",
            preview_of[&'A']
        ),
        &keys["globex:admin"],
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "another tenant's apply is invisible"
    );

    // --- Request the three undos; a repeat changes nothing --------------------------
    let mut job_id = String::new();
    for g in groups {
        let (status, view) = call(
            &router,
            "POST",
            &format!("{base_uri}/{}/restore", preview_of[&g]),
            &keys["acme:admin"],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{view}");
        assert_eq!(view["related_records_reverted"], false);
        job_id = view["job_id"].as_str().unwrap().to_owned();
    }
    let (_, again) = call(&router, "POST", &a_uri, &keys["acme:admin"], None).await;
    assert!(
        again["job_id"].is_null(),
        "a repeated undo queues nothing: {again}"
    );

    // --- Run 1 -------------------------------------------------------------------------
    let run1 = merge_restore::run(&client, &org.id, &ds, &job_id, Egress::Unguarded, limits)
        .await
        .unwrap();
    let view = |g: char| {
        let (router, key, uri) = (
            router.clone(),
            keys["acme:viewer"].clone(),
            format!("{base_uri}/{}/restore", preview_of[&g]),
        );
        async move { call(&router, "GET", &uri, &key, None).await.1 }
    };
    let a = view('A').await;
    assert_eq!(a["status"], "restored", "{a}");
    {
        let s = sf.lock().unwrap();
        let a2 = &s.records[&sfid('A', 2)];
        assert!(!a2.deleted, "A's victim is back");
        assert_eq!(
            s.records[&sfid('A', 1)].fields["Email"],
            json!("old@x.test"),
            "A's master reverted"
        );
        assert_eq!(
            s.records[&sfid('B', 1)].fields["Email"],
            json!("edited@x.test"),
            "B's later edit was not overwritten"
        );
    }
    let b = view('B').await;
    assert_eq!(b["status"], "partial", "{b}");
    assert!(b["conflicts"].to_string().contains("Email"), "{b}");
    let recreated = b["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["kind"] == "recreate")
        .unwrap();
    assert_eq!(recreated["target_id"], sfid('B', 2));
    assert!(
        recreated["new_id"].as_str().unwrap().starts_with("003Z"),
        "{b}"
    );
    let c = view('C').await;
    assert_eq!(
        c["status"], "restoring",
        "the lost undelete waits for a re-read: {c} {run1:?}"
    );
    assert!(
        a["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["kind"] == "never_assert" && s["status"] == "done"),
        "{a}"
    );

    // --- Run 2: C is verified, not undeleted again; A and B are untouched -------------
    let undeletes = |s: &Sf| s.calls.iter().filter(|c| c.starts_with("undelete")).count();
    let before = undeletes(&sf.lock().unwrap());
    let calls_before = sf.lock().unwrap().calls.len();
    merge_restore::run(&client, &org.id, &ds, &job_id, Egress::Unguarded, limits)
        .await
        .unwrap();
    let c = view('C').await;
    assert_eq!(c["status"], "restored", "{c}");
    assert!(
        c["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["kind"] == "undelete" && s["outcome"] == "verified"),
        "{c}"
    );
    assert_eq!(
        undeletes(&sf.lock().unwrap()),
        before,
        "the lost undelete was not replayed"
    );
    let after = sf.lock().unwrap().calls.clone();
    assert!(
        after[calls_before..]
            .iter()
            .all(|c| !c.contains("003A") && !c.contains("003B")),
        "{:?}",
        &after[calls_before..]
    );

    // --- Run 3: nothing open ----------------------------------------------------------
    let run3 = merge_restore::run(&client, &org.id, &ds, &job_id, Egress::Unguarded, limits)
        .await
        .unwrap();
    assert_eq!((run3.restored, run3.partial, run3.failed), (0, 0, 0));
    let history = er_entities::merge_preview_journal::read::by_preview(
        &client,
        &org.id,
        &ds,
        &preview_of[&'B'],
    )
    .await
    .unwrap();
    let path: Vec<&str> = history.iter().map(|h| h.to_status.as_str()).collect();
    assert_eq!(
        path,
        [
            "planned",
            "approved",
            "applying",
            "applied",
            "restoring",
            "partial"
        ],
        "{path:?}"
    );
}
