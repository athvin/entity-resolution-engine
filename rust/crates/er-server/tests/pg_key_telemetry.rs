//! #869: what a key mint and a key revoke leave in telemetry and the ledger.
//!
//! Four claims, each against what was actually EXPORTED or WRITTEN rather than
//! against the code that should produce it:
//!
//! 1. the request span of a mint carries `er.key_id`, `er.key_role` and the
//!    principal's `er.role`; a revoke's carries `er.key_id`;
//! 2. no span in either trace carries the plaintext, its secret half, or the
//!    stored hash — as any attribute's VALUE, so a leak is caught whatever
//!    field name it rides under (SQL spans included);
//! 3. both halves are audited, and neither audit row's detail holds the
//!    plaintext, the secret or the hash;
//! 4. each audit row's `trace_id` is the trace its request continued, so the
//!    ledger joins to the span store (`audit_rows_carry_the_requests_trace_id`
//!    proves this for `org.upsert`; this proves it for the key path).
//!
//! The recorder is the GLOBAL one for the reason `pg_orgs.rs` gives: callsite
//! interest is cached process-wide, and a thread-local subscriber flaps under
//! the concurrent pg shard. A plain `#[test]` because `pg_serial.rs` counts
//! async test attributes per file.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

use crate::common;

const MINT_TRACE: &str = "6f1c0d8e2a4b4c5d9e8f7a6b5c4d3e2f";
const REVOKE_TRACE: &str = "7a2b3c4d5e6f40718293a4b5c6d7e8f9";

fn traced(method: &str, uri: &str, trace: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer op-secret")
        .header(header::CONTENT_TYPE, "application/json")
        .header("traceparent", format!("00-{trace}-00f067aa0ba902b7-01"))
        .body(body)
        .unwrap()
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// What the mint returned: the key's id and its plaintext.
struct Minted {
    dsn: String,
    id: String,
    plaintext: String,
}

#[test]
fn a_key_mint_and_revoke_carry_ids_never_secrets() {
    let recorder = er_obs::testing::global_recorder();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let Some(minted) = rt.block_on(mint_and_revoke()) else {
        return;
    };
    let secret = minted
        .plaintext
        .splitn(3, '_')
        .nth(2)
        .expect("erk_<id>_<secret>")
        .to_string();
    let hash = pg::hash_secret(&secret);
    let secrets = [minted.plaintext.as_str(), secret.as_str(), hash.as_str()];

    let spans = recorder.captured();
    let in_trace = |trace: &str| -> Vec<&er_obs::testing::CapturedSpan> {
        spans.iter().filter(|s| s.trace_id == trace).collect()
    };
    let attr = |span: &er_obs::testing::CapturedSpan, key: &str| -> Option<String> {
        span.attributes
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };

    // 1. The ids, on the request span. Asserted before the leak scan, so a
    //    recording failure names itself instead of passing the scan vacuously.
    let mint = in_trace(MINT_TRACE);
    let mint_request = mint
        .iter()
        .find(|s| s.attributes.iter().any(|(k, _)| k == "http.route"))
        .unwrap_or_else(|| {
            panic!(
                "no request span (one carrying http.route) in the mint's trace; captured {:?}",
                mint.iter().map(|s| &s.name).collect::<Vec<_>>()
            )
        });
    assert_eq!(
        attr(mint_request, "er.key_id").as_deref(),
        Some(minted.id.as_str())
    );
    assert_eq!(
        attr(mint_request, "er.key_role").as_deref(),
        Some("steward")
    );
    assert_eq!(attr(mint_request, "er.role").as_deref(), Some("operator"));
    let revoke = in_trace(REVOKE_TRACE);
    let revoke_request = revoke
        .iter()
        .find(|s| s.attributes.iter().any(|(k, _)| k == "http.route"))
        .expect("no request span in the revoke's trace");
    assert_eq!(
        attr(revoke_request, "er.key_id").as_deref(),
        Some(minted.id.as_str())
    );
    assert!(
        mint.len() > 1,
        "only the request span was captured for the mint, so the scan below \
         would not cover the SQL spans under it: {mint:?}"
    );

    // 2. No secret anywhere in either trace.
    for span in mint.iter().chain(&revoke) {
        for (key, value) in &span.attributes {
            for s in &secrets {
                assert!(
                    !value.contains(s),
                    "span {} carries a key secret in attribute {key}",
                    span.name
                );
            }
        }
    }

    rt.block_on(ledger_assertions(&minted, &secrets));
}

/// Create an org, mint a steward key in it, revoke it. Each mutation carries
/// its own traceparent.
async fn mint_and_revoke() -> Option<Minted> {
    let db = common::fresh_db("key_telemetry").await?;
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    let resp = app
        .clone()
        .oneshot(traced(
            "POST",
            "/v1/orgs",
            "0123456789abcdef0123456789abcdef",
            Body::from(r#"{"name":"keyed","config_path":"/configs/keyed.yaml"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(traced(
            "POST",
            "/v1/orgs/keyed/api-keys",
            MINT_TRACE,
            Body::from(r#"{"role":"steward"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let issued = json_body(resp).await;
    let id = issued["id"].as_str().unwrap().to_string();
    let plaintext = issued["key"].as_str().unwrap().to_string();

    let resp = app
        .clone()
        .oneshot(traced(
            "DELETE",
            &format!("/v1/orgs/keyed/api-keys/{id}"),
            REVOKE_TRACE,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    Some(Minted { dsn, id, plaintext })
}

/// 3 and 4: the audit rows exist, carry their request's trace, and hold no
/// secret.
async fn ledger_assertions(minted: &Minted, secrets: &[&str]) {
    let client = pg::connect(&minted.dsn).await.unwrap();
    let audit = pg::audit_rows(
        &client,
        Some("keyed"),
        &pg::AuditFilters::default(),
        None,
        100,
    )
    .await
    .unwrap();
    for (action, trace) in [("key.issue", MINT_TRACE), ("key.revoke", REVOKE_TRACE)] {
        let rows: Vec<_> = audit
            .iter()
            .filter(|e| e.action == action && e.detail["key_id"] == minted.id.as_str())
            .collect();
        assert_eq!(rows.len(), 1, "expected one {action} row: {audit:?}");
        let row = rows[0];
        assert_eq!(
            row.trace_id.as_deref(),
            Some(trace),
            "the {action} audit row does not name its request's trace, so it \
             cannot be joined to the span store"
        );
        let detail = row.detail.to_string();
        for s in secrets {
            assert!(
                !detail.contains(s),
                "the {action} audit detail holds a key secret"
            );
        }
    }
}
