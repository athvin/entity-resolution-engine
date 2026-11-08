//! The Salesforce Bulk API 2.0 client against a local mock of the four
//! endpoints it speaks to — no Postgres, no lake, no network beyond loopback.
//!
//! What the mock is built to prove, beyond the happy path: the client survives
//! the three result-page behaviours a real org exhibits and a hand-written
//! concatenation would get wrong — every page repeats the CSV header, the
//! field order of a page is Salesforce's choice rather than the query's, and
//! field names come back in the FIELD'S casing, not the query's.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use er_server::salesforce::{self, Connection, SfError};

/// How many times the job status has been polled; the mock reports
/// `InProgress` first so the poll loop is actually exercised.
#[derive(Clone)]
struct Mock {
    polls: Arc<AtomicUsize>,
    /// When set, the token endpoint refuses with this body.
    refuse_token: bool,
    /// When set, job creation refuses with a Salesforce error array.
    refuse_query: bool,
}

async fn token(State(mock): State<Mock>) -> impl IntoResponse {
    if mock.refuse_token {
        return (
            StatusCode::BAD_REQUEST,
            r#"{"error":"invalid_client","error_description":"client identifier invalid"}"#,
        )
            .into_response();
    }
    (
        StatusCode::OK,
        r#"{"access_token":"tok-1","instance_url":"unused","token_type":"Bearer"}"#,
    )
        .into_response()
}

async fn create_job(State(mock): State<Mock>) -> impl IntoResponse {
    if mock.refuse_query {
        return (
            StatusCode::BAD_REQUEST,
            r#"[{"errorCode":"INVALIDENTITY","message":"entity 'Leed' is not supported"}]"#,
        )
            .into_response();
    }
    (
        StatusCode::OK,
        r#"{"id":"750JOB","state":"UploadComplete"}"#,
    )
        .into_response()
}

async fn job_status(State(mock): State<Mock>, Path(id): Path<String>) -> impl IntoResponse {
    assert_eq!(id, "750JOB");
    // One InProgress answer before JobComplete, so the backoff loop runs.
    if mock.polls.fetch_add(1, Ordering::SeqCst) == 0 {
        (StatusCode::OK, r#"{"id":"750JOB","state":"InProgress"}"#).into_response()
    } else {
        (StatusCode::OK, r#"{"id":"750JOB","state":"JobComplete"}"#).into_response()
    }
}

async fn results(
    Path(id): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    assert_eq!(id, "750JOB");
    // Page one: header + two rows, fields deliberately NOT in query order and
    // the updated-at column spelled in the field's own casing. Page two
    // (locator "p2"): the header REPEATS, one more row.
    let (body, locator) = match q.get("locator").map(String::as_str) {
        None => (
            "SystemModstamp,Email,Id\n\
             2026-11-01T00:00:00.000Z,a@x.com,L1\n\
             2026-11-03T00:00:00.000Z,b@x.com,L2\n",
            "p2",
        ),
        Some("p2") => (
            "SystemModstamp,Email,Id\n2026-11-02T00:00:00.000Z,c@x.com,L3\n",
            "null",
        ),
        Some(other) => panic!("unexpected locator {other:?}"),
    };
    ([("Sforce-Locator", locator)], body).into_response()
}

/// Serve the mock on a loopback port from its own thread + runtime; the
/// blocking client under test drives its own private runtime, so the test
/// thread itself stays synchronous.
fn serve(mock: Mock) -> String {
    let app = Router::new()
        .route("/services/oauth2/token", post(token))
        .route("/services/data/v62.0/jobs/query", post(create_job))
        .route("/services/data/v62.0/jobs/query/{id}", get(job_status))
        .route("/services/data/v62.0/jobs/query/{id}/results", get(results))
        .with_state(mock);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime for the mock");
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    std::thread::spawn(move || {
        rt.block_on(async move {
            axum::serve(listener, app).await.expect("mock serves");
        });
    });
    format!("http://{addr}")
}

fn connection(base: &str) -> Connection {
    Connection {
        instance_url: base.to_string(),
        client_id: "ck".to_string(),
        client_secret: "cs".to_string(),
        api_version: "v62.0".to_string(),
    }
}

/// The full pull: token → job → poll (with one InProgress) → two pages →
/// one CSV in OUR column order, headers deduplicated, rows re-ordered by
/// header name case-insensitively, and the watermark is the max updated-at
/// VALUE seen — page order must not matter, and page two here is older than
/// page one's newest row.
#[test]
fn a_paged_extract_lands_one_csv_in_declared_column_order() {
    let polls = Arc::new(AtomicUsize::new(0));
    let base = serve(Mock {
        polls: polls.clone(),
        refuse_token: false,
        refuse_query: false,
    });
    let columns = vec![
        "Id".to_string(),
        "SystemModstamp".to_string(),
        "Email".to_string(),
    ];
    let mut out: Vec<u8> = Vec::new();
    salesforce::write_header(&columns, &mut out).expect("header");
    let outcome = salesforce::extract_to_writer(
        &connection(&base),
        "SELECT Id, SystemModstamp, Email FROM Lead",
        &columns,
        "systemmodstamp", // the query's casing, not the field's — must still match
        &mut out,
    )
    .expect("the extract succeeds");

    assert_eq!(outcome.rows, 3);
    assert_eq!(
        outcome.max_updated.as_deref(),
        Some("2026-11-03T00:00:00.000Z"),
        "the watermark is the max VALUE across all pages, not the last page's"
    );
    assert!(polls.load(Ordering::SeqCst) >= 2, "the poll loop ran");
    let text = String::from_utf8(out).expect("utf8");
    assert_eq!(
        text,
        "Id,SystemModstamp,Email\n\
         L1,2026-11-01T00:00:00.000Z,a@x.com\n\
         L2,2026-11-03T00:00:00.000Z,b@x.com\n\
         L3,2026-11-02T00:00:00.000Z,c@x.com\n",
        "rows re-ordered into the declared column order, page headers stripped"
    );
}

/// A refused credential is [`SfError::Auth`] — the task maps it to `config`,
/// because retrying an unchanged secret cannot help.
#[test]
fn a_refused_credential_is_an_auth_error() {
    let base = serve(Mock {
        polls: Arc::new(AtomicUsize::new(0)),
        refuse_token: true,
        refuse_query: false,
    });
    let columns = vec!["Id".to_string()];
    let err = salesforce::extract_to_writer(
        &connection(&base),
        "SELECT Id FROM Lead",
        &columns,
        "SystemModstamp",
        &mut Vec::new(),
    )
    .expect_err("refused");
    match err {
        SfError::Auth(m) => assert!(m.contains("invalid_client"), "{m}"),
        other => panic!("expected Auth, got {other:?}"),
    }
}

/// A rejected query is [`SfError::Refused`] — a precondition carrying
/// Salesforce's own error code, so the job's failure text names the real
/// problem (here, a typoed sObject).
#[test]
fn a_rejected_query_is_a_refusal_naming_salesforces_error() {
    let base = serve(Mock {
        polls: Arc::new(AtomicUsize::new(0)),
        refuse_token: false,
        refuse_query: true,
    });
    let columns = vec!["Id".to_string()];
    let err = salesforce::extract_to_writer(
        &connection(&base),
        "SELECT Id FROM Leed",
        &columns,
        "SystemModstamp",
        &mut Vec::new(),
    )
    .expect_err("refused");
    match err {
        SfError::Refused(m) => assert!(m.contains("INVALIDENTITY"), "{m}"),
        other => panic!("expected Refused, got {other:?}"),
    }
}
