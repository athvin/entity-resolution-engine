//! #1012: a file larger than one import request reaches a delivery through a
//! resumable upload — open, PUT parts (out of order, retried, one wrong),
//! resume from `parts_received`, complete — and gets the SAME receipt an
//! import does. Also: an import of that file is refused at the cap, a
//! completed upload re-completes to the same receipt, a foreign dataset's
//! upload id reads as absent, a bad digest is a 422 that keeps the parts, and
//! abort deletes them.
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).
//!   ER_TEST_PG_DSN='host=localhost port=5432 user=er password=er dbname=er' \
//!     cargo test -p er-server --test main pg_bulk_upload

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use crate::common;

const DEFAULT_YAML: &str = include_str!("../../er-config/tests/fixtures/default.yaml");

async fn send(
    router: &axum::Router,
    method: &str,
    uri: &str,
    key: &str,
    content_type: &str,
    body: Body,
) -> (StatusCode, Value) {
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
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn json(
    router: &axum::Router,
    method: &str,
    uri: &str,
    key: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    send(
        router,
        method,
        uri,
        key,
        "application/json",
        body.map_or_else(Body::empty, |b| Body::from(b.to_string())),
    )
    .await
}

async fn part(
    router: &axum::Router,
    key: &str,
    url: &str,
    n: usize,
    bytes: &[u8],
) -> (StatusCode, Value) {
    send(
        router,
        "PUT",
        &url.replace("{part}", &n.to_string()),
        key,
        "application/octet-stream",
        Body::from(bytes.to_vec()),
    )
    .await
}

#[tokio::test]
async fn a_file_over_the_import_cap_arrives_through_a_resumable_upload() {
    let Some(db) = common::fresh_db("bulk_upload").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    // A 256-byte import cap: the file below is several times larger, and the
    // part size the server picks is bounded by the same cap.
    let cap = 256usize;
    let router =
        app(AppState::with_pg("op-secret".to_string(), dsn.clone()).with_max_import_bytes(cap));

    let tmp = std::env::temp_dir().join(format!("er_bulk_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let drop_root = tmp.join("drop");
    let (status, org) = json(
        &router,
        "POST",
        "/v1/orgs",
        "op-secret",
        Some(json!({
            "name": "acme",
            "config_path": tmp.join("config.yaml").to_string_lossy(),
            "drop_root": drop_root.to_string_lossy(),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{org}");
    let mut keys = Vec::new();
    for role in ["admin", "steward"] {
        let (_, k) = json(
            &router,
            "POST",
            "/v1/orgs/acme/api-keys",
            "op-secret",
            Some(json!({"role": role})),
        )
        .await;
        keys.push(k["key"].as_str().unwrap().to_owned());
    }
    let (admin, steward) = (keys[0].clone(), keys[1].clone());
    let base = "/v1/orgs/acme/datasets/default";
    let (_, v) = json(
        &router,
        "POST",
        &format!("{base}/config/versions"),
        &admin,
        Some(json!({"yaml": DEFAULT_YAML})),
    )
    .await;
    let (status, _) = json(
        &router,
        "POST",
        &format!("{base}/config/versions/{}/publish", v["version"]),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let mut csv = String::from("crm_id,first_name,last_name\n");
    for i in 0..60 {
        use std::fmt::Write as _;
        writeln!(csv, "C{i},first{i},last{i}").unwrap();
    }
    let file = csv.into_bytes();
    assert!(file.len() > cap * 3, "the file is several import caps long");
    let sha = hex::encode(Sha256::digest(&file));

    // --- 0. The plain import refuses it, which is the reason for uploads ----
    const BOUNDARY: &str = "----erbulk7f1";
    let multipart = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"crm.csv\"\r\n\
         Content-Type: text/csv\r\n\r\n{}\r\n--{BOUNDARY}--\r\n",
        String::from_utf8_lossy(&file)
    );
    let (status, _) = send(
        &router,
        "POST",
        &format!("{base}/imports?source=crm"),
        &steward,
        &format!("multipart/form-data; boundary={BOUNDARY}"),
        Body::from(multipart),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

    // --- 1. Open ---------------------------------------------------------
    let (status, refused) = json(
        &router,
        "POST",
        &format!("{base}/uploads"),
        &steward,
        Some(json!({"source": "nope", "bytes": 10, "sha256": sha})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "an undeclared source is refused up front: {refused}"
    );
    let (status, session) = json(
        &router,
        "POST",
        &format!("{base}/uploads"),
        &steward,
        Some(json!({"source": "crm", "bytes": file.len(), "sha256": sha})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{session}");
    let part_size = usize::try_from(session["part_size"].as_u64().unwrap()).unwrap();
    assert!(
        part_size <= cap,
        "a part is never larger than one request may be"
    );
    let count = usize::try_from(session["part_count"].as_u64().unwrap()).unwrap();
    assert_eq!(count, file.len().div_ceil(part_size));
    let url = session["part_url"].as_str().unwrap().to_owned();
    let upload = url.trim_end_matches("/parts/{part}").to_owned();
    let chunks: Vec<&[u8]> = file.chunks(part_size).collect();

    // --- 2. Parts out of order, one wrong, then "interrupted" ----------------
    for n in (1..=count).rev().skip(1) {
        let (status, stored) = part(&router, &steward, &url, n, chunks[n - 1]).await;
        assert_eq!(status, StatusCode::OK, "part {n}: {stored}");
        assert_eq!(stored["sha256"], hex::encode(Sha256::digest(chunks[n - 1])));
    }
    let (status, _) = part(&router, &steward, &url, count + 1, b"x").await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "out-of-range part"
    );
    let (status, _) = part(&router, &steward, &url, 1, b"short").await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a wrong-size part"
    );
    // Same size, wrong bytes: accepted as a part, caught by the digest.
    let mut corrupt = chunks[0].to_vec();
    corrupt[0] ^= 1;
    assert_eq!(
        part(&router, &steward, &url, 1, &corrupt).await.0,
        StatusCode::OK
    );

    // Completing early names exactly what is missing.
    let (status, missing) = json(
        &router,
        "POST",
        &format!("{upload}/complete"),
        &steward,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{missing}");
    assert!(
        missing["detail"]
            .as_str()
            .unwrap()
            .contains(&format!("missing: {count}")),
        "{missing}"
    );

    // --- 3. Resume: ask what arrived, send the rest ---------------------------
    let (status, state) = json(&router, "GET", &upload, &steward, None).await;
    assert_eq!(status, StatusCode::OK, "{state}");
    let have: Vec<u64> = state["parts_received"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_u64().unwrap())
        .collect();
    assert_eq!(have, (1..count as u64).collect::<Vec<_>>());
    assert_eq!(
        part(&router, &steward, &url, count, chunks[count - 1])
            .await
            .0,
        StatusCode::OK
    );
    let (status, bad) = json(
        &router,
        "POST",
        &format!("{upload}/complete"),
        &steward,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    assert!(bad["detail"].as_str().unwrap().contains("sha256"), "{bad}");
    // The parts survived the refusal; re-send the corrupt one.
    assert_eq!(
        part(&router, &steward, &url, 1, chunks[0]).await.0,
        StatusCode::OK
    );

    // --- 4. Complete: the same receipt an import returns ----------------------
    let (status, receipt) = json(
        &router,
        "POST",
        &format!("{upload}/complete"),
        &steward,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    let delivery = receipt["delivery_id"].as_str().unwrap().to_owned();
    assert_eq!(receipt["job"]["kind"], "run_all_incremental", "{receipt}");
    let job = pg::get_job(
        &client,
        receipt["job"]["org_id"].as_str().unwrap(),
        receipt["job"]["id"].as_str().unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        job.params["delivery"]["sha256"],
        sha.as_str(),
        "the run is pinned to the declared bytes"
    );
    assert_eq!(job.params["delivery"]["bytes"], file.len());
    let published =
        std::fs::read(drop_root.join("deliveries").join(format!("{delivery}.csv"))).unwrap();
    assert_eq!(published, file);
    // Its parts are reclaimed, and a lost-response retry converges.
    let upload_dir = drop_root
        .join("uploads")
        .join(upload.rsplit('/').next().unwrap());
    assert!(!upload_dir.join("part-00001").exists(), "parts reclaimed");
    let (status, again) = json(
        &router,
        "POST",
        &format!("{upload}/complete"),
        &steward,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{again}");
    assert_eq!(again["delivery_id"], delivery.as_str());
    assert_eq!(again["job"]["id"], receipt["job"]["id"]);

    // --- 5. Isolation and abort ------------------------------------------------
    let (status, created) = json(
        &router,
        "POST",
        "/v1/orgs/acme/datasets",
        "op-secret",
        Some(json!({"name": "other", "config_path": tmp.join("other.yaml").to_string_lossy(), "drop_root": tmp.join("other-drop").to_string_lossy()})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (_, second) = json(
        &router,
        "POST",
        &format!("{base}/uploads"),
        &steward,
        Some(json!({"source": "crm", "bytes": 5, "sha256": sha})),
    )
    .await;
    let second_id = second["upload_id"].as_str().unwrap();
    let (status, _) = json(
        &router,
        "GET",
        &format!("/v1/orgs/acme/datasets/other/uploads/{second_id}"),
        &steward,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "another dataset cannot see this upload"
    );
    let (status, aborted) = json(
        &router,
        "DELETE",
        &format!("{base}/uploads/{second_id}"),
        &steward,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{aborted}");
    let (status, _) = json(
        &router,
        "GET",
        &format!("{base}/uploads/{second_id}"),
        &steward,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let _ = std::fs::remove_dir_all(&tmp);
}
