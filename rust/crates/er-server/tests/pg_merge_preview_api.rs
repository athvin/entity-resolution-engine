//! #1305: merge-preview persistence and the approval API. Policies are
//! append-only revisions; publishing is idempotent on the fingerprint and
//! supersedes a group's older planned preview; approval is a compare-and-set
//! on the exact fingerprint with a journal row; stale fingerprints, decided
//! previews and replaced policies are 409; roles and scope are enforced; and
//! nothing here calls Salesforce.
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).

use std::collections::{BTreeMap, BTreeSet};

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::writeback::{
    MemberSnapshot, MergeObject, MergeProposal, PersistedProposalV1, ProposalInput, ProposalScope,
    ProviderScope,
};
use er_server::{AppState, app, pg};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::common;

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

fn input(org: &str, ds: &str, conn: &str, policy: &str, master_email: &str) -> ProposalInput {
    let provider = ProviderScope {
        org_id: org.into(),
        dataset_id: ds.into(),
        connection_id: conn.into(),
        object: MergeObject::Contact,
    };
    let member = |key: &str, id: &str, email: &str| MemberSnapshot {
        provider: provider.clone(),
        record_key: key.into(),
        external_id: id.into(),
        system_modstamp: "2026-11-27T00:00:00.000+0000".into(),
        fields: BTreeMap::from([("Email".to_string(), json!(email))]),
    };
    ProposalInput {
        scope: ProposalScope {
            provider: provider.clone(),
            entity_id: "entity-1".into(),
            lake_snapshot: 3,
            policy_revision: policy.into(),
            election_policy: er_golden::DEFAULT_MASTER_ELECTION_POLICY.into(),
        },
        master_record_key: "crm:a".into(),
        members: vec![
            member("crm:a", "003A", master_email),
            member("crm:b", "003B", "b@x.test"),
        ],
        golden_fields: BTreeMap::from([("email".to_string(), json!("b@x.test"))]),
        field_mapping: BTreeMap::from([("email".to_string(), "Email".to_string())]),
        writeback_allowlist: BTreeSet::from(["Email".to_string()]),
    }
}

#[tokio::test]
async fn previews_publish_idempotently_and_approval_binds_the_exact_proposal() {
    let Some(db) = common::fresh_db("merge_preview_api").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let router = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let mut keys = BTreeMap::new();
    for org in ["acme", "globex"] {
        let (status, created) = call(
            &router,
            "POST",
            "/v1/orgs",
            "op-secret",
            Some(json!({"name": org, "config_path": format!("/tmp/er_{org}_preview.yaml")})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        for role in ["admin", "steward", "viewer"] {
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
    let conn = pg::create_salesforce_connection(
        &client,
        &org.id,
        "prod",
        "https://example.my.salesforce.com",
        "client_credentials",
        "id",
        "secret",
        "v62.0",
    )
    .await
    .unwrap();
    let base = "/v1/orgs/acme/datasets/default";
    let policy_body = json!({
        "connection_id": conn, "object": "Contact",
        "writeback_allowlist": ["Email"], "field_mapping": {"email": "Email"},
    });

    // --- 1. Policies: admin only, validated, append-only -------------------
    let (status, _) = call(
        &router,
        "PUT",
        &format!("{base}/writeback/policies"),
        &keys["acme:steward"],
        Some(policy_body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for bad in [
        json!({"connection_id": conn, "object": "Opportunity", "writeback_allowlist": [], "field_mapping": {}}),
        json!({"connection_id": "nope", "object": "Contact", "writeback_allowlist": [], "field_mapping": {}}),
        json!({"connection_id": conn, "object": "Contact", "writeback_allowlist": ["Id"], "field_mapping": {}}),
    ] {
        let (status, refused) = call(
            &router,
            "PUT",
            &format!("{base}/writeback/policies"),
            &keys["acme:admin"],
            Some(bad),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    }
    let (status, policy) = call(
        &router,
        "PUT",
        &format!("{base}/writeback/policies"),
        &keys["acme:admin"],
        Some(policy_body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{policy}");
    let policy_id = policy["id"].as_str().unwrap().to_owned();
    let (_, listed) = call(
        &router,
        "GET",
        &format!("{base}/writeback/policies"),
        &keys["acme:viewer"],
        None,
    )
    .await;
    assert_eq!(listed["items"][0]["id"], policy_id.as_str(), "{listed}");

    // --- 2. Publishing is idempotent and supersedes ---------------------------
    let publish = |master_email: &'static str| {
        let input = input(&org.id, &ds, &conn, &policy_id, master_email);
        let proposal = MergeProposal::build(input.clone()).unwrap();
        let persisted = serde_json::to_value(PersistedProposalV1::new(input)).unwrap();
        (proposal, persisted)
    };
    let (first, first_bytes) = publish("a@x.test");
    let to_publish = |p: &MergeProposal, bytes: &Value, policy: &str| {
        let (p, bytes, policy) = (p.clone(), bytes.clone(), policy.to_owned());
        (p, bytes, policy)
    };
    let (p, bytes, pol) = to_publish(&first, &first_bytes, &policy_id);
    let row = pg::publish_preview(
        &client,
        &pg::PreviewToPublish {
            org_id: &org.id,
            dataset_id: &ds,
            origin_run_id: "run-1",
            policy_id: &pol,
            proposal: &p,
            persisted: &bytes,
            actor: "job:test",
        },
    )
    .await
    .unwrap()
    .expect("first publish stores a row");
    let again = pg::publish_preview(
        &client,
        &pg::PreviewToPublish {
            org_id: &org.id,
            dataset_id: &ds,
            origin_run_id: "run-1",
            policy_id: &pol,
            proposal: &p,
            persisted: &bytes,
            actor: "job:test",
        },
    )
    .await
    .unwrap();
    assert!(again.is_none(), "identical content is not stored twice");
    let preview = row.id.clone();
    let fingerprint = row.fingerprint.clone();
    let (status, page) = call(
        &router,
        "GET",
        &format!("{base}/merge-previews?status=planned"),
        &keys["acme:viewer"],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 1, "{page}");
    let (status, detail) = call(
        &router,
        "GET",
        &format!("{base}/merge-previews/{preview}"),
        &keys["acme:viewer"],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["summary"]["fingerprint"], fingerprint.as_str());
    assert_eq!(
        detail["field_changes"]["Email"]["after"], "b@x.test",
        "{detail}"
    );
    assert_eq!(detail["related_counts_state"], "unavailable");
    assert_eq!(detail["history"].as_array().unwrap().len(), 1, "{detail}");

    // --- 3. The decision contract ------------------------------------------
    let decide = |verb: &'static str, key: String, body: Value| {
        let router = router.clone();
        let uri = format!("{base}/merge-previews/{preview}/{verb}");
        async move { call(&router, "POST", &uri, &key, Some(body)).await }
    };
    let (status, _) = decide(
        "approve",
        keys["acme:viewer"].clone(),
        json!({"fingerprint": fingerprint}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "viewers read, stewards decide"
    );
    let (status, _) = decide(
        "approve",
        keys["acme:steward"].clone(),
        json!({"fingerprint": fingerprint, "plan": "x"}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "no executable plan or credentials ride in the body"
    );
    let (status, stale) = decide(
        "approve",
        keys["acme:steward"].clone(),
        json!({"fingerprint": "v1:not-it"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{stale}");
    let (status, approved) = decide(
        "approve",
        keys["acme:steward"].clone(),
        json!({"fingerprint": fingerprint, "reason": "same person"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["status"], "approved");
    let (status, twice) = decide(
        "approve",
        keys["acme:steward"].clone(),
        json!({"fingerprint": fingerprint}),
    )
    .await;
    assert_eq!(
        (status, twice["status"].clone()),
        (StatusCode::OK, json!("approved")),
        "idempotent"
    );
    let (status, _) = decide(
        "reject",
        keys["acme:steward"].clone(),
        json!({"fingerprint": fingerprint}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a decided preview stays decided"
    );
    let (status, foreign) = call(
        &router,
        "POST",
        &format!("/v1/orgs/globex/datasets/default/merge-previews/{preview}/approve"),
        &keys["globex:steward"],
        Some(json!({"fingerprint": fingerprint})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "another org cannot see it: {foreign}"
    );
    let (_, detail) = call(
        &router,
        "GET",
        &format!("{base}/merge-previews/{preview}"),
        &keys["acme:viewer"],
        None,
    )
    .await;
    let history: Vec<&str> = detail["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["to_status"].as_str().unwrap())
        .collect();
    assert_eq!(
        history,
        ["planned", "approved"],
        "one journal row per change, none for the no-ops"
    );

    // --- 4. A newer proposal supersedes; a replaced policy blocks approval ---
    let (second, second_bytes) = publish("old@x.test");
    let (p2, b2, pol2) = to_publish(&second, &second_bytes, &policy_id);
    let newer = pg::publish_preview(
        &client,
        &pg::PreviewToPublish {
            org_id: &org.id,
            dataset_id: &ds,
            origin_run_id: "run-2",
            policy_id: &pol2,
            proposal: &p2,
            persisted: &b2,
            actor: "job:test",
        },
    )
    .await
    .unwrap()
    .unwrap();
    let (third, third_bytes) = publish("older@x.test");
    let (p3, b3, pol3) = to_publish(&third, &third_bytes, &policy_id);
    pg::publish_preview(
        &client,
        &pg::PreviewToPublish {
            org_id: &org.id,
            dataset_id: &ds,
            origin_run_id: "run-3",
            policy_id: &pol3,
            proposal: &p3,
            persisted: &b3,
            actor: "job:test",
        },
    )
    .await
    .unwrap()
    .unwrap();
    let (_, superseded) = call(
        &router,
        "GET",
        &format!("{base}/merge-previews/{}", newer.id),
        &keys["acme:viewer"],
        None,
    )
    .await;
    assert_eq!(
        superseded["summary"]["status"], "superseded",
        "{superseded}"
    );
    let (_, planned) = call(
        &router,
        "GET",
        &format!("{base}/merge-previews?status=planned"),
        &keys["acme:viewer"],
        None,
    )
    .await;
    let current = planned["items"][0].clone();
    assert_eq!(planned["items"].as_array().unwrap().len(), 1, "{planned}");
    let (status, _) = call(
        &router,
        "PUT",
        &format!("{base}/writeback/policies"),
        &keys["acme:admin"],
        Some(policy_body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, blocked) = call(
        &router,
        "POST",
        &format!(
            "{base}/merge-previews/{}/approve",
            current["id"].as_str().unwrap()
        ),
        &keys["acme:steward"],
        Some(json!({"fingerprint": current["fingerprint"]})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "built under a replaced policy: {blocked}"
    );
    assert!(
        blocked["detail"].as_str().unwrap().contains("policy"),
        "{blocked}"
    );
}
