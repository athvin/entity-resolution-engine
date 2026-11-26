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

    // --- 6b. Human review and tags through the same contract (#516) ----------
    // The admin mints a steward key for the people who review (#866).
    let (status, issued) = json_call(
        &router,
        "POST",
        "/v1/orgs/acme/api-keys",
        &key,
        Some(json!({"role": "steward"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let steward = issued["key"].as_str().unwrap().to_owned();
    let (status, reviews) =
        json_call(&router, "GET", &format!("{base}/reviews"), &steward, None).await;
    assert_eq!(status, StatusCode::OK, "{reviews}");
    let open = reviews["items"].as_array().unwrap();
    assert!(
        !open.is_empty(),
        "base_10 leaves gray-band pairs to review: {reviews}"
    );
    let review = open[0]["id"].as_str().unwrap().to_owned();
    // Reject it, applied now: the decision is staged, drained and reconciled.
    let (status, decided) = json_call(
        &router,
        "POST",
        &format!("{base}/reviews/bulk-resolve"),
        &steward,
        Some(
            json!({"items": [{"review_id": review, "resolution": "no_match"}], "apply_now": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{decided}");
    let applied = settle(
        &mut harness,
        &client,
        &org.id,
        &[decided["apply_job"].as_str().unwrap().to_owned()],
    )
    .await;
    assert_eq!(applied[0].1, "succeeded", "{applied:?}");
    let (status, after) = json_call(
        &router,
        "GET",
        &format!("{base}/reviews/{review}"),
        &steward,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert!(
        after["status"]
            .as_str()
            .is_some_and(|s| s.starts_with("resolved")),
        "the decision is recorded on the review: {after}"
    );

    // Unmatch one member of a duplicate group: it leaves that group.
    let member = groups[0]["members"][0].as_str().unwrap().to_owned();
    let (status, unmatched) = json_call(
        &router,
        "POST",
        &format!("{base}/records/unmatch"),
        &steward,
        Some(json!({"record_key": member, "apply_now": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{unmatched}");
    assert_eq!(unmatched["entity_id"], entity, "{unmatched}");
    let applied = settle(
        &mut harness,
        &client,
        &org.id,
        &[unmatched["apply_job"].as_str().unwrap().to_owned()],
    )
    .await;
    assert_eq!(applied[0].1, "succeeded", "{applied:?}");
    let (_, regrouped) = json_call(
        &router,
        "GET",
        &format!("{base}/duplicates"),
        &steward,
        None,
    )
    .await;
    assert!(
        !regrouped["items"].as_array().unwrap().iter().any(|g| {
            g["entity_id"] == entity
                && g["members"]
                    .as_array()
                    .is_some_and(|m| m.iter().any(|k| k == &json!(member)))
        }),
        "the unmatched record left its group: {regrouped}"
    );
    // Lineage of what happened: the latest run's entity events.
    let (status, runs) = json_call(&router, "GET", &format!("{base}/runs"), &steward, None).await;
    assert_eq!(status, StatusCode::OK, "{runs}");
    // A bare array of runs; the correction pass is the one the decisions made.
    let runs = runs.as_array().unwrap();
    let latest = runs
        .iter()
        .find(|r| r["mode"] == "correct")
        .unwrap_or(&runs[0])["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, events) = json_call(
        &router,
        "GET",
        &format!("{base}/runs/{latest}/entity-events"),
        &steward,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events}");
    assert!(
        !events["items"].as_array().unwrap().is_empty(),
        "the correction run recorded entity events: {events}"
    );

    // Tags: an admin defines the vocabulary, a steward applies it.
    let (status, tag) = json_call(
        &router,
        "POST",
        "/v1/orgs/acme/tags",
        &key,
        Some(json!({"name": "vip"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{tag}");
    let (status, tagged) = json_call(
        &router,
        "POST",
        &format!("{base}/tags/{}/apply", tag["id"].as_str().unwrap()),
        &steward,
        Some(json!({"subject_type": "entity", "subject_id": entity})),
    )
    .await;
    assert!(status.is_success(), "{status} {tagged}");
    let (status, taggings) = json_call(
        &router,
        "GET",
        &format!("{base}/taggings?subject_type=entity&subject_id={entity}"),
        &steward,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{taggings}");
    assert!(taggings.to_string().contains(entity), "{taggings}");

    // --- 7. A dataset of the tenant's own, end to end (#1010-#1012) ----------
    // Created, validated, configured and fed with the admin key alone.
    let (status, provisioning) = json_call(
        &router,
        "POST",
        "/v1/orgs/acme/datasets",
        &key,
        Some(json!({"name": "contacts", "display_name": "Contacts", "entity_type": "person"})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{provisioning}");
    let provisioned = settle(
        &mut harness,
        &client,
        &org.id,
        &[provisioning["job_id"].as_str().unwrap().to_owned()],
    )
    .await;
    assert_eq!(provisioned[0].1, "succeeded", "{provisioned:?}");
    let contacts = "/v1/orgs/acme/datasets/contacts";
    // A mapping mistake is reported before anything is stored or queued.
    let (status, checked) = json_call(
        &router,
        "POST",
        &format!("{contacts}/config/validate"),
        &key,
        Some(json!({"yaml": STARTER_YAML.replace("given_name:   fname", "givenname:   fname")})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{checked}");
    assert_eq!(checked["valid"], false, "{checked}");
    assert!(checked.to_string().contains("givenname"), "{checked}");
    let (status, checked) = json_call(
        &router,
        "POST",
        &format!("{contacts}/config/validate"),
        &key,
        Some(json!({"yaml": STARTER_YAML})),
    )
    .await;
    assert_eq!(checked["valid"], true, "{status} {checked}");
    let (status, draft) = json_call(
        &router,
        "POST",
        &format!("{contacts}/config/versions"),
        &key,
        Some(json!({"yaml": STARTER_YAML})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{draft}");
    let (status, published) = json_call(
        &router,
        "POST",
        &format!("{contacts}/config/versions/{}/publish", draft["version"]),
        &key,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{published}");
    settle(
        &mut harness,
        &client,
        &org.id,
        &ids(&published["jobs_enqueued"]),
    )
    .await;

    // JSON with an Idempotency-Key: the retry is the same delivery.
    let crm = csv_records(&fixture("base_10/base/crm.csv"))
        .into_iter()
        .map(|mut r| {
            r.as_object_mut().unwrap().remove("persona_id");
            r
        })
        .collect::<Vec<_>>();
    let push = |idem: &'static str| {
        let router = router.clone();
        let key = key.clone();
        let crm = crm.clone();
        async move {
            let resp = router
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/orgs/acme/datasets/contacts/records/ingest")
                        .header(header::AUTHORIZATION, format!("Bearer {key}"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .header("Idempotency-Key", idem)
                        .body(Body::from(
                            json!({"source": "crm", "records": crm}).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap())
        }
    };
    let (status, first) = push("contacts-crm-1").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{first}");
    let (_, retried) = push("contacts-crm-1").await;
    assert_eq!(
        retried["job"]["id"], first["job"]["id"],
        "a retry is the same run"
    );
    let mut fed = vec![first["job"]["id"].as_str().unwrap().to_owned()];

    // billing through a resumable upload, webforms as a plain import.
    let billing = std::fs::read(fixture("base_10/base/billing.csv")).unwrap();
    let sha = {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(&billing))
    };
    let (status, session) = json_call(
        &router,
        "POST",
        &format!("{contacts}/uploads"),
        &key,
        Some(json!({"source": "billing", "bytes": billing.len(), "sha256": sha})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{session}");
    let part_size = usize::try_from(session["part_size"].as_u64().unwrap()).unwrap();
    for (i, chunk) in billing.chunks(part_size).enumerate() {
        let (status, _, _) = call(
            &router,
            "PUT",
            &session["part_url"]
                .as_str()
                .unwrap()
                .replace("{part}", &(i + 1).to_string()),
            &key,
            "application/octet-stream",
            Body::from(chunk.to_vec()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, receipt) = json_call(
        &router,
        "POST",
        &format!(
            "{contacts}/uploads/{}/complete",
            session["upload_id"].as_str().unwrap()
        ),
        &key,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    fed.push(receipt["job"]["id"].as_str().unwrap().to_owned());
    let content = std::fs::read_to_string(fixture("base_10/base/webforms.csv")).unwrap();
    let (ct, body) = multipart("webforms.csv", &content);
    let (status, bytes, _) = call(
        &router,
        "POST",
        &format!("{contacts}/imports?source=webforms"),
        &key,
        &ct,
        body,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let reply: Value = serde_json::from_slice(&bytes).unwrap();
    fed.push(reply["job"]["id"].as_str().unwrap().to_owned());
    let after = settle(&mut harness, &client, &org.id, &fed).await;
    assert!(after.iter().all(|(_, s, _)| s == "succeeded"), "{after:?}");

    // The results agree with the default dataset's, and the export agrees
    // with the live read at the snapshot it pinned.
    let (_, bytes, _) = call(
        &router,
        "GET",
        &format!("{contacts}/golden-records"),
        &key,
        "application/json",
        Body::empty(),
    )
    .await;
    let reader =
        arrow::ipc::reader::StreamReader::try_new(std::io::Cursor::new(bytes), None).unwrap();
    assert_eq!(reader.map(|b| b.unwrap().num_rows()).sum::<usize>(), 10);
    // One lineage row per source record delivered: every fixture row.
    let source_records: usize = ["crm", "billing", "webforms"]
        .iter()
        .map(|s| csv_records(&fixture(&format!("base_10/base/{s}.csv"))).len())
        .sum();
    for (kind, expected) in [("golden", 10usize), ("lineage", source_records)] {
        let (status, queued) = json_call(
            &router,
            "POST",
            &format!("{contacts}/exports"),
            &key,
            Some(json!({"kind": kind, "format": "jsonl"})),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{kind}: {queued}");
        let export = queued["export_id"].as_str().unwrap().to_owned();
        let done = settle(
            &mut harness,
            &client,
            &org.id,
            std::slice::from_ref(&export),
        )
        .await;
        assert_eq!(done[0].1, "succeeded", "{kind}: {done:?}");
        let (status, bytes, _) = call(
            &router,
            "GET",
            &format!("{contacts}/exports/{export}/download"),
            &key,
            "application/json",
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{kind}");
        let rows = String::from_utf8(bytes)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count();
        assert_eq!(rows, expected, "{kind} export rows");
    }

    // A delivery the pipeline cannot read fails as a run, visibly: the
    // receipt's job reports `failed` with a class and a detail.
    let (ct, body) = multipart("webforms.csv", "not_a_declared_header\nx\n");
    let (status, bytes, _) = call(
        &router,
        "POST",
        &format!("{contacts}/imports?source=webforms"),
        &key,
        &ct,
        body,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let broken: Value = serde_json::from_slice(&bytes).unwrap();
    let broken_job = broken["job"]["id"].as_str().unwrap().to_owned();
    settle(
        &mut harness,
        &client,
        &org.id,
        std::slice::from_ref(&broken_job),
    )
    .await;
    let (status, job) = json_call(
        &router,
        "GET",
        &format!("{contacts}/jobs/{broken_job}"),
        &key,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{job}");
    assert_eq!(job["state"], "failed", "{job}");
    assert!(job["error_class"].as_str().is_some(), "{job}");
    assert!(job["error_detail"].as_str().is_some(), "{job}");

    // Datasets do not leak into each other: contacts' upload and export ids
    // are absent from the default dataset.
    for uri in [
        format!("{base}/uploads/{}", session["upload_id"].as_str().unwrap()),
        format!("{base}/jobs/{broken_job}"),
    ] {
        let (status, _) = json_call(&router, "GET", &uri, &key, None).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "cross-dataset reads are 404: {uri}"
        );
    }

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
