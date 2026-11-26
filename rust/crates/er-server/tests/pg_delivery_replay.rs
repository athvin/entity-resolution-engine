//! #1011: a delivery retried with the same `Idempotency-Key` is the SAME
//! delivery. Same key + same bytes hands back the original receipt (delivery
//! id and job) and queues nothing new; same key + different bytes is a 409;
//! the key is scoped to one dataset and source; unsupported modes are refused
//! by name. Both delivery paths — JSON push and file upload — are exercised.
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).
//!   ER_TEST_PG_DSN='host=localhost port=5432 user=er password=er dbname=er' \
//!     cargo test -p er-server --test main pg_delivery_replay

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::common;

const DEFAULT_YAML: &str = include_str!("../../er-config/tests/fixtures/default.yaml");

struct Reply {
    status: StatusCode,
    body: Value,
    replayed: bool,
}

async fn send(
    router: &axum::Router,
    uri: &str,
    key: &str,
    content_type: &str,
    idempotency: Option<&str>,
    body: Body,
) -> Reply {
    let mut req = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {key}"))
        .header(header::CONTENT_TYPE, content_type);
    if let Some(k) = idempotency {
        req = req.header("Idempotency-Key", k);
    }
    let resp = router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let replayed = resp.headers().get("idempotent-replayed").is_some();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    Reply {
        status,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        replayed,
    }
}

async fn push(router: &axum::Router, key: &str, idem: Option<&str>, body: Value) -> Reply {
    send(
        router,
        "/v1/orgs/acme/datasets/default/records/ingest",
        key,
        "application/json",
        idem,
        Body::from(body.to_string()),
    )
    .await
}

async fn upload(
    router: &axum::Router,
    key: &str,
    idem: Option<&str>,
    query: &str,
    csv: &str,
) -> Reply {
    const BOUNDARY: &str = "----erreplay41c2";
    let body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"crm.csv\"\r\n\
         Content-Type: text/csv\r\n\r\n{csv}\r\n--{BOUNDARY}--\r\n"
    );
    send(
        router,
        &format!("/v1/orgs/acme/datasets/default/imports?{query}"),
        key,
        &format!("multipart/form-data; boundary={BOUNDARY}"),
        idem,
        Body::from(body),
    )
    .await
}

fn deliveries(drop_root: &std::path::Path) -> usize {
    std::fs::read_dir(drop_root.join("deliveries")).map_or(0, |d| {
        d.filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".csv")
        })
        .count()
    })
}

#[tokio::test]
async fn a_retried_delivery_is_the_same_delivery() {
    let Some(db) = common::fresh_db("delivery_replay").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let router = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    let tmp = std::env::temp_dir().join(format!("er_replay_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let drop_root = tmp.join("drop");
    let org = send(
        &router,
        "/v1/orgs",
        "op-secret",
        "application/json",
        None,
        Body::from(
            json!({
                "name": "acme",
                "config_path": tmp.join("config.yaml").to_string_lossy(),
                "drop_root": drop_root.to_string_lossy(),
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(org.status, StatusCode::CREATED, "{}", org.body);
    let mut keys = Vec::new();
    for role in ["admin", "steward"] {
        let r = send(
            &router,
            "/v1/orgs/acme/api-keys",
            "op-secret",
            "application/json",
            None,
            Body::from(json!({ "role": role }).to_string()),
        )
        .await;
        keys.push(r.body["key"].as_str().unwrap().to_owned());
    }
    let (admin, steward) = (&keys[0], &keys[1]);
    let base = "/v1/orgs/acme/datasets/default/config/versions";
    let v = send(
        &router,
        base,
        admin,
        "application/json",
        None,
        Body::from(json!({ "yaml": DEFAULT_YAML }).to_string()),
    )
    .await;
    let published = send(
        &router,
        &format!("{base}/{}/publish", v.body["version"]),
        admin,
        "application/json",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(published.status, StatusCode::OK, "{}", published.body);

    let records = json!({
        "source": "crm",
        "records": [{"crm_id": "C1", "first_name": "ada"}, {"crm_id": "C2", "first_name": "alan"}],
    });

    // --- 1. JSON push: same key + same records is the same delivery --------
    let first = push(&router, steward, Some("order-17"), records.clone()).await;
    assert_eq!(first.status, StatusCode::ACCEPTED, "{}", first.body);
    assert!(!first.replayed);
    let again = push(&router, steward, Some("order-17"), records.clone()).await;
    assert_eq!(again.status, StatusCode::ACCEPTED, "{}", again.body);
    assert!(again.replayed, "a replay says so");
    assert_eq!(again.body["delivery_id"], first.body["delivery_id"]);
    assert_eq!(again.body["job"]["id"], first.body["job"]["id"]);
    assert_eq!(deliveries(&drop_root), 1, "the replay's copy was discarded");

    // Same key, different records: a misuse, refused before a second run.
    let mut changed = records.clone();
    changed["records"][0]["first_name"] = json!("grace");
    let conflict = push(&router, steward, Some("order-17"), changed).await;
    assert_eq!(conflict.status, StatusCode::CONFLICT, "{}", conflict.body);
    assert!(
        conflict.body["detail"]
            .as_str()
            .unwrap()
            .contains(first.body["delivery_id"].as_str().unwrap()),
        "the refusal names the delivery the key belongs to: {}",
        conflict.body
    );
    assert_eq!(deliveries(&drop_root), 1);

    // The key is scoped to a source: the same key on billing is new data.
    let billing = push(
        &router,
        steward,
        Some("order-17"),
        json!({"source": "billing", "records": [{"account_no": "B1", "fname": "ada"}]}),
    )
    .await;
    assert_eq!(billing.status, StatusCode::ACCEPTED, "{}", billing.body);
    assert!(!billing.replayed);
    assert_ne!(billing.body["job"]["id"], first.body["job"]["id"]);

    // Without a key every request is its own delivery, as before.
    let a = push(&router, steward, None, records.clone()).await;
    let b = push(&router, steward, None, records.clone()).await;
    assert_ne!(a.body["delivery_id"], b.body["delivery_id"]);

    // --- 2. File upload: the same contract -------------------------------
    let csv = "crm_id,first_name\nC9,edsger\n";
    let up = upload(&router, steward, Some("file-1"), "source=crm", csv).await;
    assert_eq!(up.status, StatusCode::ACCEPTED, "{}", up.body);
    let up_again = upload(&router, steward, Some("file-1"), "source=crm", csv).await;
    assert!(up_again.replayed, "{}", up_again.body);
    assert_eq!(up_again.body["job"]["id"], up.body["job"]["id"]);
    let up_conflict = upload(
        &router,
        steward,
        Some("file-1"),
        "source=crm",
        "crm_id,first_name\nC9,dijkstra\n",
    )
    .await;
    assert_eq!(
        up_conflict.status,
        StatusCode::CONFLICT,
        "{}",
        up_conflict.body
    );

    // --- 3. Refusals before anything is staged ----------------------------
    let full = upload(&router, steward, None, "source=crm&mode=full_refresh", csv).await;
    assert_eq!(
        full.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        full.body
    );
    assert!(
        full.body["detail"]
            .as_str()
            .unwrap()
            .contains("not supported")
    );
    let mut full_push = records.clone();
    full_push["mode"] = json!("full_refresh");
    assert_eq!(
        push(&router, steward, None, full_push).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut upsert = records.clone();
    upsert["mode"] = json!("upsert");
    assert_eq!(
        push(&router, steward, Some("order-18"), upsert)
            .await
            .status,
        StatusCode::ACCEPTED
    );
    let bad_key = push(&router, steward, Some("has space"), records.clone()).await;
    assert_eq!(
        bad_key.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        bad_key.body
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
