//! #1013's launch proof: a new tenant configures, ingests, resolves and reads
//! back results through tenant-public HTTP alone, with no Salesforce.
//!
//! The operator does exactly one thing, `POST /v1/orgs`, which is the
//! onboarding step that hands the customer its one-time admin key (#866 owns
//! making that self-serve). From then on every request carries that key, and
//! nothing here uses SQL, the operator token, a drop directory or the `er` CLI.
//! The dispatcher runs the jobs through the real executor, one pod per task.
//!
//! Gated on `ER_TEST_PG_DSN` + `DEDUPE_E2E_DUCKLAKE=1`; one `#[tokio::test]`
//! (`pg_serial.rs`).

use std::path::{Path, PathBuf};

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

/// A CSV fixture as the JSON records a customer would push.
fn csv_records(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap();
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().unwrap().split(',').collect();
    lines
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let row: serde_json::Map<String, Value> = header
                .iter()
                .zip(line.split(','))
                .map(|(k, v)| ((*k).to_owned(), json!(v)))
                .collect();
            Value::Object(row)
        })
        .collect()
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
async fn a_tenant_configures_ingests_resolves_and_reads_back_with_its_own_key() {
    let Some(db) = common::fresh_db("launch_proof").await else {
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
    let root = std::env::temp_dir().join(format!("er_launch_proof_{}", std::process::id()));
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

    // Settle the jobs the API handed back, by id, and report each.
    // (`run_to_settlement` claims any queued job while it pumps, so watching
    // the queue would miss jobs that ran alongside another.)
    async fn settle(
        harness: &mut Harness,
        client: &tokio_postgres::Client,
        org_id: &str,
        ids: &[String],
    ) -> Vec<(String, String, Option<String>)> {
        let mut out = Vec::new();
        for id in ids {
            harness.run_to_settlement(client, org_id, id).await;
            let job = pg::get_job(client, org_id, id).await.unwrap().unwrap();
            out.push((job.kind, job.state, job.error_detail));
        }
        out
    }
    let ids = |v: &Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_owned())
            .collect()
    };

    // --- 1. Configure: publish a customer edit to the starter config ------
    // The starter already maps the canonical attributes onto this customer's
    // own column names (`fname`, `billing_email`, `addr1`, ...).
    let (status, active) = json_call(&router, "GET", &format!("{base}/config"), &key, None).await;
    assert_eq!(status, StatusCode::OK, "{active}");
    let yaml = STARTER_YAML.replace("priority_rank: 3", "priority_rank: 4");
    let (status, version) = json_call(
        &router,
        "POST",
        &format!("{base}/config/versions"),
        &key,
        Some(json!({"yaml": yaml})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{version}");
    let v = version["version"].as_i64().unwrap();
    let (status, published) = json_call(
        &router,
        "POST",
        &format!("{base}/config/versions/{v}/publish"),
        &key,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{published}");
    assert_eq!(
        published["jobs_enqueued"].as_array().map(Vec::len),
        Some(2),
        "{published}"
    );
    let after_publish = settle(
        &mut harness,
        &client,
        &org.id,
        &ids(&published["jobs_enqueued"]),
    )
    .await;
    let rebuild = after_publish
        .iter()
        .find(|(kind, ..)| kind == "run_all_full");
    assert_eq!(
        rebuild.map(|r| r.1.as_str()),
        Some("succeeded"),
        "{after_publish:?}"
    );
    // On a never-ingested dataset there is nothing to train on, so the
    // publish-time `train` settles as a no-op (#1186), not a failure.
    let train = after_publish.iter().find(|(kind, ..)| kind == "train");
    assert_eq!(
        train.map(|r| r.1.as_str()),
        Some("succeeded"),
        "{after_publish:?}"
    );
    // ...and registers no model: there was nothing to train.
    let (status, models) = json_call(&router, "GET", &format!("{base}/models"), &key, None).await;
    assert_eq!(status, StatusCode::OK, "{models}");
    assert!(
        !models.to_string().contains("trained-"),
        "no-op train registered a model: {models}"
    );

    // --- 2. Ingest --------------------------------------------------------
    // An undeclared column is refused, with the declared list in the error.
    let crm = csv_records(&fixture("base_10/base/crm.csv"));
    let (status, refused) = json_call(
        &router,
        "POST",
        &format!("{base}/records/ingest"),
        &key,
        Some(json!({"source": "crm", "records": crm})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(
        refused["detail"].as_str().unwrap().contains("persona_id"),
        "{refused}"
    );
    // The fixture's `persona_id` is test truth, not customer data: drop it.
    let crm: Vec<Value> = crm
        .into_iter()
        .map(|mut r| {
            r.as_object_mut().unwrap().remove("persona_id");
            r
        })
        .collect();
    let (status, pushed) = json_call(
        &router,
        "POST",
        &format!("{base}/records/ingest"),
        &key,
        Some(json!({"source": "crm", "records": crm})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{pushed}");
    assert!(pushed["delivery_id"].as_str().is_some(), "{pushed}");
    let mut delivered = vec![pushed["job"]["id"].as_str().unwrap().to_owned()];

    let upload = |source: &'static str| {
        let router = router.clone();
        let key = key.clone();
        async move {
            let content =
                std::fs::read_to_string(fixture(&format!("base_10/base/{source}.csv"))).unwrap();
            let (ct, body) = multipart(&format!("{source}.csv"), &content);
            let (status, bytes, _) = call(
                &router,
                "POST",
                &format!("/v1/orgs/acme/datasets/default/imports?source={source}"),
                &key,
                &ct,
                body,
            )
            .await;
            let reply: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            assert_eq!(status, StatusCode::ACCEPTED, "{source}: {reply}");
            reply
        }
    };
    for source in ["billing", "webforms"] {
        let reply = upload(source).await;
        delivered.push(reply["job"]["id"].as_str().unwrap().to_owned());
    }
    let after_ingest = settle(&mut harness, &client, &org.id, &delivered).await;
    assert!(
        !after_ingest.is_empty()
            && after_ingest
                .iter()
                .all(|(_, state, _)| state == "succeeded"),
        "{after_ingest:?}"
    );

    // --- 3. Follow the receipts --------------------------------------------
    let (status, imports) = json_call(&router, "GET", &format!("{base}/imports"), &key, None).await;
    assert_eq!(status, StatusCode::OK, "{imports}");
    let sources: std::collections::BTreeSet<&str> = imports["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["source_system"].as_str())
        .collect();
    assert_eq!(sources, ["billing", "crm", "webforms"].into(), "{imports}");
    let (status, jobs) = json_call(&router, "GET", &format!("{base}/jobs"), &key, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        jobs["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|j| j["state"] != "queued"),
        "{jobs}"
    );

    // --- 4. Read the results -----------------------------------------------
    let (status, dups) = json_call(&router, "GET", &format!("{base}/duplicates"), &key, None).await;
    assert_eq!(status, StatusCode::OK, "{dups}");
    let groups = dups["items"].as_array().unwrap();
    assert!(!groups.is_empty(), "base_10 has duplicate personas: {dups}");
    assert!(
        groups.iter().all(|g| g["member_count"].as_i64() >= Some(2)),
        "{dups}"
    );
    // Pagination: one group per page, following the cursor to the end.
    let mut seen = 0;
    let mut cursor: Option<String> = None;
    loop {
        let uri = match &cursor {
            Some(c) => format!("{base}/duplicates?limit=1&cursor={c}"),
            None => format!("{base}/duplicates?limit=1"),
        };
        let (status, page) = json_call(&router, "GET", &uri, &key, None).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        seen += page["items"].as_array().unwrap().len();
        match page["next_cursor"].as_str() {
            Some(c) if seen < 100 => cursor = Some(c.to_owned()),
            _ => break,
        }
    }
    assert_eq!(seen, groups.len(), "paging visits every group exactly once");

    let (status, bytes, headers) = call(
        &router,
        "GET",
        &format!("{base}/golden-records"),
        &key,
        "application/json",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get("x-er-snapshot").is_some());
    let reader =
        arrow::ipc::reader::StreamReader::try_new(std::io::Cursor::new(bytes), None).unwrap();
    let golden: usize = reader.map(|b| b.unwrap().num_rows()).sum();
    assert_eq!(golden, 10, "base_10 resolves to ten entities");

    // Lineage for one resolved entity.
    let entity = groups[0]["entity_id"].as_str().unwrap();
    let (status, detail) = json_call(
        &router,
        "GET",
        &format!("{base}/golden-records/{entity}"),
        &key,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert!(
        detail
            .to_string()
            .contains(groups[0]["members"][0].as_str().unwrap()),
        "{detail}"
    );

    // --- 5. A redelivery is accepted and changes nothing --------------------
    let reply = upload("billing").await;
    let again = settle(
        &mut harness,
        &client,
        &org.id,
        &[reply["job"]["id"].as_str().unwrap().to_owned()],
    )
    .await;
    assert!(
        again.iter().all(|(_, state, _)| state == "succeeded"),
        "{again:?}"
    );
    let (_, bytes, _) = call(
        &router,
        "GET",
        &format!("{base}/golden-records"),
        &key,
        "application/json",
        Body::empty(),
    )
    .await;
    let reader =
        arrow::ipc::reader::StreamReader::try_new(std::io::Cursor::new(bytes), None).unwrap();
    assert_eq!(reader.map(|b| b.unwrap().num_rows()).sum::<usize>(), 10);

    // --- 6. Another tenant's key sees nothing of this one --------------------
    let (status, other) = json_call(
        &router,
        "POST",
        "/v1/orgs",
        "op-secret",
        Some(json!({"name": "globex"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{other}");
    let foreign = other["admin_key"].as_str().unwrap();
    let (status, _) = json_call(&router, "GET", &format!("{base}/duplicates"), foreign, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "cross-org reads are 404, not 403"
    );

    let _ = std::fs::remove_dir_all(&root);
}
