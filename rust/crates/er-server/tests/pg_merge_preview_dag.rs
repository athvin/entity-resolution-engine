//! #993: the merge-preview job end to end over a real lake. A crm delivery
//! with an in-source duplicate pair is provenanced as a Salesforce extract; a
//! matching webforms row is a plain upload. Previews propose merging the crm
//! pair only, the upload never joins a group, regenerating changes nothing,
//! and the checkpoint advances past the runs read.
//!
//! Gated on `ER_TEST_PG_DSN` + `DEDUPE_E2E_DUCKLAKE=1`; one test (`pg_serial.rs`).

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::provision::{Mode, Profile};
use er_server::{AppState, app, pg};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::common;
use crate::executor_harness::Harness;

const STARTER_YAML: &str = include_str!("../../er-cli/tests/fixtures/test.yaml");
const LEXICON: &str = include_str!("../../er-cli/tests/fixtures/nickname_variants.csv");

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/static")
        .join(rel)
}

/// One tenant-public request. `key` is the customer's own API key.
async fn call(
    router: &axum::Router,
    method: &str,
    uri: &str,
    key: &str,
    content_type: &str,
    body: Body,
) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, content_type)
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 << 20)
        .await
        .unwrap()
        .to_vec();
    (status, bytes, headers)
}

async fn json_call(
    router: &axum::Router,
    method: &str,
    uri: &str,
    key: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
    let (status, bytes, _) = call(router, method, uri, key, "application/json", body).await;
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

fn multipart(filename: &str, content: &str) -> (String, Body) {
    const BOUNDARY: &str = "----erlaunchproof7a1d";
    let body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
         Content-Type: text/csv\r\n\r\n{content}\r\n--{BOUNDARY}--\r\n"
    );
    (
        format!("multipart/form-data; boundary={BOUNDARY}"),
        Body::from(body),
    )
}

#[tokio::test]
async fn previews_come_only_from_provider_provenanced_members() {
    let Some(db) = common::fresh_db("merge_preview_dag").await else {
        return;
    };
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping the launch proof (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    // The operator's deployment profile: where tenants live, the starter
    // config, and the shared model artifacts. This is server configuration,
    // not anything the customer sends.
    let root = std::env::temp_dir().join(format!("er_preview_dag_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let template = root.join("starter.yaml");
    std::fs::write(&template, STARTER_YAML).unwrap();
    let lexicon = root.join("lexicon.csv");
    std::fs::write(&lexicon, LEXICON).unwrap();
    let tf = root.join("model_test_v1.tf.csv");
    std::fs::copy(fixture("model_test_v1.tf.csv"), &tf).unwrap();
    let profile = Profile {
        mode: Mode::Local,
        root: root.clone(),
        config_template: template.clone(),
        maint_dsn: None,
        data_template: None,
        env_extra: serde_json::from_value(json!({
            "ER_DUCKDB_THREADS": "2",
            "ER_LEXICON_PATH": lexicon.to_string_lossy(),
            "ER_MODEL_PATH": fixture("model_test_v1.json").to_string_lossy(),
            "ER_TF_PATH": tf.to_string_lossy(),
            "ER_MODEL_VERSION": "v0001",
            "ER_TF_SNAPSHOT_ID": "01JWMDTFSNAP00000000000001",
        }))
        .unwrap(),
    };
    let router = app(AppState::with_pg("op-secret".to_string(), dsn.clone())
        .with_provision(Ok(Some(profile.clone()))));
    let mut harness = Harness::start_with_profile(&dsn, root.clone(), Ok(Some(profile))).await;

    // --- Onboarding: the one operator call -------------------------------
    let (status, created) = json_call(
        &router,
        "POST",
        "/v1/orgs",
        "op-secret",
        Some(json!({"name": "acme"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let key = created["admin_key"].as_str().unwrap().to_owned();
    let org = pg::get_org(&client, "acme").await.unwrap().unwrap();
    let provision = created["job_id"].as_str().unwrap().to_owned();
    let settled = harness
        .run_to_settlement(&client, &org.id, &provision)
        .await;
    assert_eq!(settled, "succeeded", "provisioning");

    let base = "/v1/orgs/acme/datasets/default";
    async fn settle(
        harness: &mut Harness,
        client: &tokio_postgres::Client,
        org_id: &str,
        id: &str,
    ) -> (String, Option<String>) {
        harness.run_to_settlement(client, org_id, id).await;
        let job = pg::get_job(client, org_id, id).await.unwrap().unwrap();
        (job.state, job.error_detail)
    }

    // Deliveries: crm carries an in-source duplicate (C001 / C901), webforms
    // the same person again.
    let crm = "crm_id,first_name,last_name,email_address,phone,street_address,city,state,zip,dob,last_modified\n\
C001,Robert,Chen,robert.chen@example.com,(628) 555-0101,1420 Judah Street Apt 5,San Francisco,CA,94107,1985-03-14,2024-01-05 08:12:00\n\
C901,Robert,Chen,robert.chen@example.com,(628) 555-0101,1420 Judah St Apt 5,San Francisco,CA,94107,1985-03-14,2024-03-01 09:00:00\n\
C002,Priya,Raghavan,priya.raghavan@example.com,(415) 555-0132,88 Harbor Street #4B,San Francisco,CA,94110,1990-07-22,2024-01-09 11:00:00\n";
    let webforms = std::fs::read_to_string(fixture("base_10/base/webforms.csv")).unwrap();
    let mut runs = std::collections::BTreeMap::new();
    for (source, content) in [("crm", crm.to_string()), ("webforms", webforms)] {
        let (ct, body) = multipart(&format!("{source}.csv"), &content);
        let (status, bytes, _) = call(
            &router,
            "POST",
            &format!("{base}/imports?source={source}"),
            &key,
            &ct,
            body,
        )
        .await;
        let reply: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status, StatusCode::ACCEPTED, "{reply}");
        let job = reply["job"]["id"].as_str().unwrap().to_owned();
        let settled = settle(&mut harness, &client, &org.id, &job).await;
        assert_eq!(settled.0, "succeeded", "{source}: {settled:?}");
        runs.insert(
            source,
            (job, reply["delivery_id"].as_str().unwrap().to_owned()),
        );
    }
    let ds = pg::default_dataset_id(&client, &org.id)
        .await
        .unwrap()
        .unwrap();
    // The crm run is provenanced as an extract would record it (#1296).
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
    pg::record_provenance(
        &client,
        &pg::DeliveryProvenance {
            org_id: &org.id,
            dataset_id: &ds,
            run_id: &runs["crm"].0,
            delivery_id: &runs["crm"].1,
            source: "crm",
            connection_id: &conn,
            object: "Contact",
            delivery_sha256: "d",
            source_sha256: "s",
        },
    )
    .await
    .unwrap();

    let generate = |key: String| {
        let router = router.clone();
        async move {
            let (s, b, _) = call(
                &router,
                "POST",
                "/v1/orgs/acme/datasets/default/merge-previews/generate",
                &key,
                "application/json",
                Body::empty(),
            )
            .await;
            (
                s,
                serde_json::from_slice::<Value>(&b).unwrap_or(Value::Null),
            )
        }
    };
    // No policy yet: the job finds nothing to do.
    let (status, queued) = generate(key.clone()).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");
    let settled = settle(
        &mut harness,
        &client,
        &org.id,
        queued["job_id"].as_str().unwrap(),
    )
    .await;
    assert_eq!(settled.0, "succeeded", "{settled:?}");

    let (status, policy) = json_call(
        &router,
        "PUT",
        &format!("{base}/writeback/policies"),
        &key,
        Some(json!({
            "connection_id": conn, "object": "Contact",
            "writeback_allowlist": ["email_address", "first_name"],
            "field_mapping": {"email": "email_address", "given_name": "first_name"},
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{policy}");
    // An auto-approval rule in force before generation (#997).
    let (status, rule) = json_call(
        &router,
        "PUT",
        &format!("{base}/writeback/auto-approval"),
        &key,
        Some(json!({"connection_id": conn, "object": "Contact", "enabled": true, "min_score": 0.5, "max_members": 5})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rule}");
    let rule_id = rule["id"].as_str().unwrap().to_owned();
    // An upsert policy for records that arrived by upload (#998).
    let (status, upsert_policy) = json_call(
        &router,
        "PUT",
        &format!("{base}/writeback/upsert-policies"),
        &key,
        Some(json!({"connection_id": conn, "object": "Contact", "external_id_field": "DupeZero_Id__c",
            "field_mapping": {"email": "Email", "family_name": "LastName"},
            "writeback_allowlist": ["Email", "LastName"], "sources": ["webforms"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{upsert_policy}");

    // --- Generate: only the provenanced crm pair forms a group -------------
    let (status, queued) = generate(key.clone()).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");
    let settled = settle(
        &mut harness,
        &client,
        &org.id,
        queued["job_id"].as_str().unwrap(),
    )
    .await;
    assert_eq!(settled.0, "succeeded", "{settled:?}");
    let (status, page) = json_call(
        &router,
        "GET",
        &format!("{base}/merge-previews"),
        &key,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "one group: the crm pair: {page}");
    assert_eq!(items[0]["member_count"], 2, "{page}");
    assert_eq!(items[0]["object"], "Contact");
    // The pair was scored by the match run, so the group has evidence, and
    // the rule approved it as it was published.
    let score = items[0]["score"].as_f64().expect("a scored group");
    assert!(score >= 0.5, "{page}");
    assert_eq!(items[0]["status"], "approved", "{page}");
    assert_eq!(
        items[0]["decided_by"],
        format!("policy:{rule_id}"),
        "{page}"
    );
    // Upserts: the webforms-only entities are proposed; the entity already
    // synced from the Contact object is a merge, never an upsert.
    let merged_entity = items[0]["entity_id"].as_str().unwrap().to_owned();
    let (status, upserts) = json_call(
        &router,
        "GET",
        &format!("{base}/upsert-previews"),
        &key,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{upserts}");
    let proposed = upserts["items"].as_array().unwrap();
    assert!(
        !proposed.is_empty(),
        "webforms-only entities are proposed: {upserts}"
    );
    assert!(
        proposed
            .iter()
            .all(|u| u["entity_id"] != merged_entity.as_str()),
        "{upserts}"
    );
    // Keyed by the entity id; only allowlisted fields, and only golden values
    // that exist (a record with no golden email writes no Email).
    assert!(
        proposed
            .iter()
            .all(|u| u["external_id_value"] == u["entity_id"]
                && !u["fields"].as_object().unwrap().is_empty()
                && u["fields"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .all(|k| k == "Email" || k == "LastName")),
        "{upserts}"
    );
    let id = items[0]["id"].as_str().unwrap().to_owned();
    let (_, detail) = json_call(
        &router,
        "GET",
        &format!("{base}/merge-previews/{id}"),
        &key,
        None,
    )
    .await;
    let text = detail["proposal"].to_string();
    assert!(text.contains("C001") && text.contains("C901"), "{detail}");
    assert!(
        !text.contains("webforms"),
        "an uploaded row never joins a group: {detail}"
    );
    assert_eq!(
        detail["proposal"]["input"]["scope"]["provider"]["connection_id"],
        conn.as_str()
    );

    // --- Regenerating reads nothing new and changes nothing ------------------
    let checkpoint = pg::preview_watermark(&client, &org.id, &ds).await.unwrap();
    assert!(
        checkpoint.is_some(),
        "the checkpoint advanced past the runs read"
    );
    let (_, queued) = generate(key.clone()).await;
    let settled = settle(
        &mut harness,
        &client,
        &org.id,
        queued["job_id"].as_str().unwrap(),
    )
    .await;
    assert_eq!(settled.0, "succeeded", "{settled:?}");
    let (_, again) = json_call(
        &router,
        "GET",
        &format!("{base}/merge-previews"),
        &key,
        None,
    )
    .await;
    assert_eq!(again["items"].as_array().unwrap().len(), 1, "{again}");
    assert_eq!(
        pg::preview_watermark(&client, &org.id, &ds).await.unwrap(),
        checkpoint
    );

    let _ = std::fs::remove_dir_all(&root);
}
