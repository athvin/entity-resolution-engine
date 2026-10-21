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

/// Every event type this system can emit, with what it means.
///
/// This exists because a subscription was previously an unvalidated free-form
/// array: `webhooks_for_delivery` just tests whether the subscriber's `events`
/// list contains the fired type, so a tenant could subscribe to
/// `entity.merged`, or to `job.succeeded` with the typo, and wait forever for a
/// delivery that was never going to arrive. Nothing told them, and nothing
/// could — there was no list of what exists.
///
/// Adding an event means adding it here, which is also what makes it
/// subscribable. A type absent from this list is refused at registration
/// instead of accepted and silently never delivered.
pub const EVENT_CATALOG: [(&str, &str); 3] = [
    (
        "job.succeeded",
        "A queued job finished successfully. Fired once per OUTCOME, not per \
         attempt — a job that failed twice and then succeeded delivers one \
         success.",
    ),
    (
        "job.failed",
        "A job failed terminally, after exhausting retries if its failure was \
         retryable. The payload carries the job's `error_class`, so a subscriber \
         can tell a missing input (precondition) from a real fault.",
    ),
    (
        "job.canceled",
        "An operator canceled the job. Until this release cancel delivered \
         nothing at all, so a system waiting on an outcome waited forever.",
    ),
];

// Deliberately NOT listed: `job.running`. The dispatcher marks a job running
// but never emits for it, and listing an event nothing fires is the exact bug
// this catalog exists to prevent. Add it here when something emits it, not
// before.

/// Whether `event_type` is one this system can actually emit.
#[must_use]
pub fn is_known_event(event_type: &str) -> bool {
    EVENT_CATALOG.iter().any(|(name, _)| *name == event_type)
}

/// The catalog as a JSON array, for the discovery endpoint.
#[must_use]
pub fn catalog_json() -> Value {
    Value::Array(
        EVENT_CATALOG
            .iter()
            .map(|(name, description)| json!({ "event": name, "description": description }))
            .collect(),
    )
}

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

/// How long a single delivery may take before it is abandoned. A subscriber
/// that accepts the connection and then stalls would otherwise hold a
/// dispatcher thread for `reqwest`'s default, which is forever.
const DELIVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The HTTP client every webhook delivery must use.
///
/// `reqwest::Client::new()` follows up to ten redirects, which is a complete
/// bypass of the URL guard: register `https://public.example/` and have it 302
/// to `http://169.254.169.254/latest/meta-data/`, and the guard only ever
/// inspected the first hop. Redirects are therefore refused outright rather
/// than re-checked per hop — a webhook endpoint has no reason to redirect, so
/// the simpler rule is also the right one, and a 3xx surfaces as a status the
/// tenant can see instead of a silent fetch somewhere else.
///
/// `egress_guard.rs` forbids `reqwest::Client::new()` anywhere in this crate's
/// sources so a future caller cannot quietly reintroduce the default policy.
#[must_use]
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(DELIVERY_TIMEOUT)
        .build()
        // The builder only fails on TLS backend initialisation, which would
        // break every outbound request anyway; falling back to the default
        // would silently restore redirect-following, so this must not be
        // papered over.
        .expect("a reqwest client with redirects disabled")
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
    emit_with_egress(
        pg_client,
        http,
        org_id,
        org_name,
        event_type,
        data,
        crate::urlguard::Egress::Guarded,
    )
    .await
}

/// [`emit`] with the outbound-URL guard made explicit.
///
/// `Egress::Unguarded` exists for ONE caller — the delivery integration test,
/// which has to POST to a listener on `127.0.0.1` to observe that a delivery
/// really happens. `tests/egress_guard.rs` fails if any file under
/// `er-server/src` names this function or `Egress::Unguarded`, so the seam
/// cannot quietly become a production path. There is deliberately no
/// environment variable or config flag: a guard that configuration can switch
/// off is a guard that gets found off.
///
/// # Errors
/// Propagates only the Postgres subscriber lookup error.
#[doc(hidden)]
pub async fn emit_with_egress(
    pg_client: &tokio_postgres::Client,
    http: &reqwest::Client,
    org_id: &str,
    org_name: &str,
    event_type: &str,
    data: &Value,
    egress: crate::urlguard::Egress,
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
        // Re-checked HERE, not only at registration. A hostname that resolved
        // to a public address when the webhook was created can resolve to a
        // private one now — that is DNS rebinding, and a registration-time-only
        // guard does nothing about it.
        if let Err(rejection) = crate::urlguard::check(&url, egress).await {
            results.push(DeliveryResult {
                webhook_id,
                url,
                status: None,
                error: Some(format!("refused: {rejection}")),
            });
            continue;
        }
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
    emit_job_event_with_egress(pg_client, http, job, crate::urlguard::Egress::Guarded).await
}

/// [`emit_job_event`] with the outbound-URL guard made explicit; see
/// [`emit_with_egress`] for why this exists and what forbids its use in
/// product code.
///
/// # Errors
/// Propagates the Postgres subscriber lookup error.
#[doc(hidden)]
pub async fn emit_job_event_with_egress(
    pg_client: &tokio_postgres::Client,
    http: &reqwest::Client,
    job: &crate::Job,
    egress: crate::urlguard::Egress,
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
    emit_with_egress(
        pg_client,
        http,
        &job.org_id,
        &org_name,
        &event_type,
        &data,
        egress,
    )
    .await
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
