//! A slow tenant catalog must not prevent the control plane answering probes.
//! Uses a real DuckDB PostgreSQL attach held at the socket boundary, rather than
//! a sleep inserted into product code. The observer runs outside Tokio so a
//! starved executor cannot delay its deadline and turn the test falsely green.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

use crate::common;

fn probe(address: SocketAddr, path: &str) -> std::io::Result<bool> {
    let timeout = Duration::from_secs(2);
    let mut stream = TcpStream::connect_timeout(&address, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response.starts_with("HTTP/1.1 200 "))
}

/// Wait until DuckDB is actually blocked on catalog I/O, then probe the API
/// before releasing that I/O. A deadline bounds the test even if attach fails
/// before connecting (for example, if a required extension is missing).
fn observe(catalog: TcpListener, api: SocketAddr) -> (bool, bool) {
    catalog.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let held = loop {
        match catalog.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "DuckDB never opened the catalog");
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accepting the catalog connection: {error}"),
        }
    };
    let live = probe(api, "/healthz").unwrap_or(false);
    let ready = probe(api, "/readyz").unwrap_or(false);
    // Release even on probe failure: the request then reports the expected
    // catalog error instead of leaving a blocked thread behind after the test.
    drop(held);
    drop(catalog);
    (live, ready)
}

#[tokio::test]
async fn dashboard_catalog_wait_does_not_starve_probes() {
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping lake-read readiness test (set DEDUPE_E2E_DUCKLAKE=1)");
        return;
    }
    let Some(db) = common::fresh_db("readiness_reads").await else {
        return;
    };
    let mut client = pg::connect(db.dsn()).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let router = app(AppState::with_pg("op-secret".into(), db.dsn().into()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = listener.local_addr().unwrap();
    let http_app = router.clone();
    let server = tokio::spawn(async move { axum::serve(listener, http_app).await.unwrap() });

    for endpoint in ["metrics", "data-quality", "runs"] {
        let catalog = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = catalog.local_addr().unwrap().port();
        let env = serde_json::json!({
            "ER_LAKE_CATALOG": format!(
                "postgres:host=127.0.0.1 port={port} user=fixture dbname=fixture sslmode=disable connect_timeout=10"
            ),
        });
        pg::upsert_org_byo(&client, endpoint, "/unused.yaml", &env, None, None, None)
            .await
            .unwrap();
        pg::converge_default_datasets(&mut client).await.unwrap();
        let observer = std::thread::spawn(move || observe(catalog, api));
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/orgs/{endpoint}/datasets/default/{endpoint}"))
                    .header("Authorization", "Bearer op-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let probes = observer.join().unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(probes, (true, true), "{endpoint} blocked health/readiness");
    }
    server.abort();
}
