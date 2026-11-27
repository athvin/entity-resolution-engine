//! #1309: the merge-preview job over an ACCOUNT lake, end to end. The
//! `acct_base` corpus (crm + vendors, the same companies spelled two ways) is
//! imported into an `entity_type: account` tenant, both runs are provenanced as
//! Salesforce Account extracts through one connection, and an Account policy
//! maps the account golden columns. Previews must elect over
//! `acct_int_std_records` and carry `acct_golden_records` values. Before #1309
//! the job read only the person relations and settled as a no-op.
//!
//! Gated on `ER_TEST_PG_DSN` + `DEDUPE_E2E_DUCKLAKE=1`; one test (`pg_serial.rs`).

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::common;
use crate::executor_harness::Harness;

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/static")
        .join(rel)
}

async fn call(
    router: &axum::Router,
    method: &str,
    uri: &str,
    key: &str,
    content_type: &str,
    body: Body,
) -> (StatusCode, Vec<u8>) {
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
    let bytes = axum::body::to_bytes(resp.into_body(), 64 << 20)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

async fn json_call(
    router: &axum::Router,
    method: &str,
    uri: &str,
    key: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
    let (status, bytes) = call(router, method, uri, key, "application/json", body).await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn multipart(filename: &str, content: &str) -> (String, Body) {
    const BOUNDARY: &str = "----eracctpreview9c2e";
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
#[allow(clippy::too_many_lines)]
async fn account_datasets_produce_merge_previews_from_account_goldens() {
    let Some(db) = common::fresh_db("merge_preview_acct_dag").await else {
        return;
    };
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping the account preview DAG (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    // An account tenant: the acct_base config and the hand-authored account
    // model, a lake of its own in a temp tree.
    let tmp = std::env::temp_dir().join(format!("er_acct_preview_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    std::fs::create_dir_all(tmp.join("drop")).unwrap();
    let config_path = tmp.join("account.yaml");
    std::fs::copy(fixture("acct_base/config.yaml"), &config_path).unwrap();
    let tf_path = tmp.join("acct_model_v1.tf.csv");
    std::fs::copy(fixture("acct_model_v1.tf.csv"), &tf_path).unwrap();
    let env = json!({
        "ER_LAKE_CATALOG": tmp.join("catalog.ducklake").to_string_lossy(),
        "ER_LAKE_DATA_PATH": tmp.join("data").to_string_lossy(),
        "ER_CONFIG_PATH": config_path.to_string_lossy(),
        "ER_MODEL_PATH": fixture("acct_model_v1.json").to_string_lossy(),
        "ER_TF_PATH": tf_path.to_string_lossy(),
        "ER_MODEL_VERSION": "acct_v1",
        "ER_TF_SNAPSHOT_ID": "ACCTTFSNAP0000000000000001",
    });
    pg::upsert_org_byo(
        &client,
        "acme",
        config_path.to_str().unwrap(),
        &env,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    pg::converge_default_datasets(&mut client).await.unwrap();
    let org_id = pg::org_id_by_name(&client, "acme")
        .await
        .unwrap()
        .expect("org");
    let (_, key) = pg::issue_key(&client, &org_id, "admin").await.unwrap();

    let router = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let mut harness = Harness::start(&dsn, tmp.clone()).await;
    // An account dataset, created by the operator with its own lake and the
    // account model (a dataset's entity type is fixed at creation).
    let (status, created) = json_call(
        &router,
        "POST",
        "/v1/orgs/acme/datasets",
        "op-secret",
        Some(json!({
            "name": "accts", "display_name": "Accounts", "entity_type": "account",
            "config_path": config_path.to_string_lossy(), "env": env,
            "drop_root": tmp.join("drop").to_string_lossy(),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let ds = pg::get_dataset(&client, &org_id, "accts")
        .await
        .unwrap()
        .expect("the account dataset")
        .id;
    let base = "/v1/orgs/acme/datasets/accts";

    // The account config, published through the API as a tenant would.
    let yaml = std::fs::read_to_string(fixture("acct_base/config.yaml")).unwrap();
    let (status, draft) = json_call(
        &router,
        "POST",
        &format!("{base}/config/versions"),
        &key,
        Some(json!({"yaml": yaml})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{draft}");
    let (status, published) = json_call(
        &router,
        "POST",
        &format!("{base}/config/versions/{}/publish", draft["version"]),
        &key,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{published}");
    for job in published["jobs_enqueued"].as_array().into_iter().flatten() {
        let job = job
            .as_str()
            .map(str::to_owned)
            .or_else(|| job["id"].as_str().map(str::to_owned))
            .unwrap();
        let settled = harness.run_to_settlement(&client, &org_id, &job).await;
        assert_eq!(settled, "succeeded", "publish job {job}");
    }

    // --- Deliveries ------------------------------------------------------------
    // crm is the Salesforce Account extract, with an in-source duplicate (c1 /
    // c9, the same company spelled two ways). vendors is a plain upload of the
    // same companies: it has no provenance, so it never joins a group.
    let crm = "crm_id,company,website,phone,street,city,state,zip,last_modified\n\
c1,Acme Inc.,https://www.acme.com,(415) 555-0100,1 Main St,Springfield,CA,90001,2026-01-01\n\
c9,ACME Inc,www.acme.com,415-555-0100,1 Main Street,Springfield,CA,90001,2026-03-01\n\
c2,Globex Corporation,http://globex.com,415-555-0200,2 Oak Ave,Metropolis,NY,10001,2026-01-02\n";
    let vendors = std::fs::read_to_string(fixture("acct_base/vendors.csv")).unwrap();
    let mut runs = std::collections::BTreeMap::new();
    for (source, content) in [("crm", crm.to_string()), ("vendors", vendors)] {
        let (ct, body) = multipart(&format!("{source}.csv"), &content);
        let (status, bytes) = call(
            &router,
            "POST",
            &format!("{base}/imports?source={source}"),
            &key,
            &ct,
            body,
        )
        .await;
        let reply: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status, StatusCode::ACCEPTED, "{source}: {reply}");
        let job = reply["job"]["id"].as_str().unwrap().to_owned();
        let settled = harness.run_to_settlement(&client, &org_id, &job).await;
        let detail = pg::get_job(&client, &org_id, &job)
            .await
            .unwrap()
            .and_then(|j| j.error_detail);
        assert_eq!(settled, "succeeded", "{source}: {detail:?}");
        runs.insert(
            source,
            (job, reply["delivery_id"].as_str().unwrap().to_owned()),
        );
    }

    // The crm run is provenanced as an Account extract would record it (#1296).
    let conn = pg::create_salesforce_connection(
        &client,
        &org_id,
        "prod",
        "https://example.my.salesforce.com",
        "client_credentials",
        "id",
        "secret",
        "v62.0",
    )
    .await
    .unwrap();
    for source in ["crm"] {
        pg::record_provenance(
            &client,
            &pg::DeliveryProvenance {
                org_id: &org_id,
                dataset_id: &ds,
                run_id: &runs[source].0,
                delivery_id: &runs[source].1,
                source,
                connection_id: &conn,
                object: "Account",
                delivery_sha256: "d",
                source_sha256: "s",
            },
        )
        .await
        .unwrap();
    }

    // A person column is refused for an Account policy, and the account
    // golden columns are accepted.
    let (status, refused) = json_call(
        &router,
        "PUT",
        &format!("{base}/writeback/policies"),
        &key,
        Some(json!({
            "connection_id": conn, "object": "Account",
            "writeback_allowlist": ["company"], "field_mapping": {"given_name": "company"},
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(refused.to_string().contains("company_name"), "{refused}");
    let (status, policy) = json_call(
        &router,
        "PUT",
        &format!("{base}/writeback/policies"),
        &key,
        Some(json!({
            "connection_id": conn, "object": "Account",
            "writeback_allowlist": ["company", "website"],
            "field_mapping": {"company_name": "company", "domain": "website"},
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{policy}");

    // --- Generate ------------------------------------------------------------
    let (status, bytes) = call(
        &router,
        "POST",
        &format!("{base}/merge-previews/generate"),
        &key,
        "application/json",
        Body::empty(),
    )
    .await;
    let queued: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");
    let job = queued["job_id"].as_str().unwrap().to_owned();
    let settled = harness.run_to_settlement(&client, &org_id, &job).await;
    let detail = pg::get_job(&client, &org_id, &job)
        .await
        .unwrap()
        .and_then(|j| j.error_detail);
    assert_eq!(settled, "succeeded", "{detail:?}");

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
    assert_eq!(items[0]["object"], "Account", "{page}");
    assert_eq!(items[0]["member_count"], 2, "{page}");

    // Every proposal is built from the account mart: a company name and a
    // domain from acct_golden_records, mapped onto the policy's fields, with
    // the master elected among the group's own members.
    for item in items {
        let id = item["id"].as_str().unwrap();
        let (_, detail) = json_call(
            &router,
            "GET",
            &format!("{base}/merge-previews/{id}"),
            &key,
            None,
        )
        .await;
        let input = &detail["proposal"]["input"];
        assert_eq!(input["scope"]["provider"]["object"], "Account", "{detail}");
        let golden = input["golden_fields"].as_object().expect("golden fields");
        assert!(
            golden.contains_key("company_name") && golden.contains_key("domain"),
            "account golden values: {detail}"
        );
        assert!(
            !golden.contains_key("given_name"),
            "no person column: {detail}"
        );
        let master = input["master_record_key"].as_str().unwrap();
        let members: Vec<&str> = input["members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["record_key"].as_str().unwrap())
            .collect();
        assert!(
            members.contains(&master),
            "the master is a group member: {detail}"
        );
        let ids: Vec<&str> = input["members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["external_id"].as_str().unwrap())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, ["c1", "c9"], "only the provenanced pair: {detail}");
    }

    let _ = std::fs::remove_dir_all(&tmp);
}
