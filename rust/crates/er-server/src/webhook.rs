//! Outbound webhook delivery (the send side of `webhooks.py`): sign an event
//! payload with the subscriber's HMAC-SHA256 secret and POST it to the
//! registered URL. Registration (CRUD) lives in `lib.rs`/`pg.rs`; this is the
//! delivery. [`emit_job_event`] is the in-process emit the dispatcher fires
//! after a job settles — a standalone outbox-draining daemon (retries,
//! backoff) is the infra-bound piece and is not built here.

use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use crate::pg;

/// The header carrying the payload signature: `sha256=<hex>` (GitHub-style, so
/// a subscriber recomputes HMAC-SHA256 over the raw body to authenticate it).
pub const SIGNATURE_HEADER: &str = "X-ER-Signature";

type HmacSha256 = Hmac<Sha256>;

/// `sha256=<hex HMAC-SHA256(secret, body)>` for the delivered `body`.
#[must_use]
pub fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// One delivery's outcome: the HTTP status (if the POST completed) or the
/// transport error. A non-2xx status is reported via `status`, not `error`.
#[derive(Debug)]
pub struct DeliveryResult {
    pub webhook_id: String,
    pub url: String,
    pub status: Option<u16>,
    pub error: Option<String>,
}

/// POST `body` to `url` with the signature + JSON content-type headers.
async fn post(
    http: &reqwest::Client,
    url: &str,
    body: Vec<u8>,
    signature: &str,
) -> Result<u16, String> {
    let resp = http
        .post(url)
        .header(SIGNATURE_HEADER, signature)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok(resp.status().as_u16())
}

/// Fan an event out to every active webhook subscribed to `event_type`: build
/// the body `{event, org, ...data}`, sign it per-subscriber (each has its own
/// secret), and POST. Returns one [`DeliveryResult`] per target (empty when
/// none subscribe). A dead endpoint is recorded, never propagated, so one
/// failure can't suppress the others.
///
/// # Errors
/// Propagates only the Postgres subscriber lookup error; per-delivery transport
/// failures are captured in the returned results.
pub async fn emit(
    pg_client: &tokio_postgres::Client,
    http: &reqwest::Client,
    org_id: &str,
    org_name: &str,
    event_type: &str,
    data: &Value,
) -> Result<Vec<DeliveryResult>, tokio_postgres::Error> {
    let targets = pg::webhooks_for_delivery(pg_client, org_id, event_type).await?;

    let mut body_doc = json!({ "event": event_type, "org": org_name });
    if let (Some(obj), Some(extra)) = (body_doc.as_object_mut(), data.as_object()) {
        for (k, v) in extra {
            obj.insert(k.clone(), v.clone());
        }
    }
    let body = serde_json::to_vec(&body_doc).unwrap_or_default();

    let mut results = Vec::with_capacity(targets.len());
    for (webhook_id, url, secret) in targets {
        let signature = sign(&secret, &body);
        let (status, error) = match post(http, &url, body.clone(), &signature).await {
            Ok(code) => (Some(code), None),
            Err(e) => (None, Some(e)),
        };
        results.push(DeliveryResult {
            webhook_id,
            url,
            status,
            error,
        });
    }
    Ok(results)
}

/// Emit the `job.<state>` event for a settled job to the org's subscribers.
///
/// # Errors
/// Propagates the Postgres subscriber lookup error.
pub async fn emit_job_event(
    pg_client: &tokio_postgres::Client,
    http: &reqwest::Client,
    job: &crate::Job,
) -> Result<Vec<DeliveryResult>, tokio_postgres::Error> {
    let event_type = format!("job.{}", job.state);
    let data = json!({
        "job_id": job.id,
        "kind": job.kind,
        "state": job.state,
    });
    // Subscribers key on the org's id; the payload names the org (the public
    // handle), resolved from the job's org_id.
    let org_name = crate::pg::get_org_by_id(pg_client, &job.org_id)
        .await?
        .map_or_else(|| job.org_id.clone(), |o| o.name);
    emit(pg_client, http, &job.org_id, &org_name, &event_type, &data).await
}

#[cfg(test)]
mod tests {
    use super::sign;

    #[test]
    fn signature_is_deterministic_and_keyed() {
        let body = br#"{"event":"job.succeeded"}"#;
        let a = sign("secret-a", body);
        assert!(a.starts_with("sha256="));
        // Same key + body -> same signature; a different key -> different.
        assert_eq!(a, sign("secret-a", body));
        assert_ne!(a, sign("secret-b", body));
        // A changed body changes the signature.
        assert_ne!(a, sign("secret-a", br#"{"event":"job.failed"}"#));
    }
}
