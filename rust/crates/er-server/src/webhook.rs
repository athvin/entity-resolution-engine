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
/// could — there was no list of what exists. (`entity.merged` has a producer as
/// of O3c-2; `entity.edge_cut` is the one that still does not, and is refused
/// for exactly the same reason the typo is.)
///
/// Adding an event means adding it here, which is also what makes it
/// subscribable. A type absent from this list is refused at registration
/// instead of accepted and silently never delivered.
pub const EVENT_CATALOG: [(&str, &str); 12] = [
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
    (
        "config.published",
        "A config version became the active one. Carries the version, the change \
         TIER and the blocks that moved, so a subscriber can tell a threshold \
         tweak (tier A, which re-reads existing scores) from a comparison \
         change (tier C, which retrains and re-resolves the corpus) without \
         diffing YAML.",
    ),
    (
        "import.received",
        "A delivery landed and its run was queued. Fired when the file is \
         committed to the drop root, not when the run finishes — those are \
         different questions, and `job.succeeded` already answers the second, \
         keyed by the `job_id` this payload carries.",
    ),
    (
        "review.resolved",
        "A steward resolved a review and the decision was APPLIED to the lake. \
         Not fired at the moment of resolution: a resolution is staged and \
         drained later, and a subscriber acting on an intent the engine has not \
         enforced yet would read a corpus that disagrees with the event. \
         Carries the verdict, including a dismissal.",
    ),
    (
        "entity.created",
        "A run created entities that did not exist before. AGGREGATED: one \
         event per kind per run, carrying the count and `detail_path`, not one \
         event per entity — a run over a large corpus writes millions of ledger \
         rows and a POST each would be worse than none.",
    ),
    (
        "entity.member_added",
        "Records joined entities that already existed. Aggregated per run like \
         every `entity.*` event. This is the kind that tells a downstream \
         system a golden record it already holds has changed underneath it, \
         without it having to diff the whole corpus.",
    ),
    (
        "entity.member_removed",
        "Records left entities that survived. Aggregated per run. Paired with \
         `member_added` this is the honest description of a re-resolution: the \
         partition moved, and these are the two directions it moved in.",
    ),
    (
        "entity.merged",
        "Entities were merged into one. Aggregated per run. The event a \
         subscriber most often wants, and the one a tenant could previously \
         subscribe to and wait forever for — it was the worked example in this \
         catalog's own reason for existing.",
    ),
    (
        "entity.split",
        "An entity separated into several. Aggregated per run. Fires for a \
         steward `never` assertion taking effect and for a re-resolution the \
         corpus no longer supports, which are the same fact to a subscriber.",
    ),
    (
        "entity.retired",
        "Entities stopped existing — every member moved elsewhere. Aggregated \
         per run. A subscriber holding a reference to one needs to know it is \
         now a tombstone rather than absent.",
    ),
];

// Deliberately NOT listed: `job.running`. The dispatcher marks a job running
// but never emits for it, and listing an event nothing fires is the exact bug
// this catalog exists to prevent. Add it here when something emits it, not
// before.
//
// Also deliberately NOT listed: `entity.edge_cut`. `edge_cut` is in the
// ledger's own kind list (`er_schema::model::EVENT_TYPES`) but nothing writes
// it — the `cut_edges` relation has no producer either — so it falls under the
// same rule as `job.running`. `dispatcher::entity_event_name` returns `None`
// for it and logs the count, so if it ever gains a writer that shows up as an
// operator-visible line rather than as events quietly never arriving.

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

/// How many times a delivery is attempted before it is given up on. Six with
/// the backoff in `webhook_delivery/update/settle.sql` spans about ten minutes,
/// which covers a subscriber restart without keeping a dead endpoint in the
/// queue for hours.
pub const MAX_DELIVERY_ATTEMPTS: i64 = 6;

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

/// Record an event for every active webhook subscribed to `event_type`, and
/// return without sending anything.
///
/// This used to POST inline and every caller discarded the result with
/// `let _ =`, so a subscriber that was unreachable for a moment missed the
/// event **permanently** and nobody — tenant or operator — could find out: the
/// per-target outcomes were computed and thrown away. Now the event lands in
/// `webhook_deliveries` before any network call, and [`drain_once`] delivers it
/// with retries.
///
/// The body is built and stored once, here, rather than rebuilt per attempt:
/// a retry has to deliver the same bytes the signature is computed over, and
/// the log has to show what was actually sent.
///
/// Returns one row per subscriber that will be attempted (empty when none
/// subscribe).
///
/// # Errors
/// Propagates the Postgres subscriber lookup or insert error. Unlike the old
/// inline path this one CAN fail meaningfully — failing to record an event is
/// worth knowing about, where failing to reach one subscriber is not.
pub async fn emit(
    pg_client: &tokio_postgres::Client,
    org_id: &str,
    org_name: &str,
    event_type: &str,
    data: &Value,
) -> Result<Vec<er_entities::webhook_delivery::WebhookDelivery>, tokio_postgres::Error> {
    let targets = pg::webhooks_for_delivery(pg_client, org_id, event_type).await?;
    if targets.is_empty() {
        return Ok(Vec::new());
    }

    let mut body_doc = json!({ "event": event_type, "org": org_name });
    if let (Some(obj), Some(extra)) = (body_doc.as_object_mut(), data.as_object()) {
        for (k, v) in extra {
            obj.insert(k.clone(), v.clone());
        }
    }

    let mut queued = Vec::with_capacity(targets.len());
    for (webhook_id, _url, _secret) in targets {
        let row = er_entities::webhook_delivery::create::enqueue(
            pg_client,
            &er_ids::new_v7(),
            org_id,
            &webhook_id,
            event_type,
            &body_doc,
            MAX_DELIVERY_ATTEMPTS,
        )
        .await?;
        queued.extend(row);
    }
    Ok(queued)
}

/// Deliver one due outbox row, with retries. `Ok(None)` when nothing is due.
///
/// The drainer, not the emit site, is where the network lives. That split is
/// what makes a transient failure recoverable: the event is already durable, so
/// a 500 or a refused connection becomes a backoff rather than a loss.
///
/// Classification decides retry-or-give-up, and getting it wrong wastes either
/// deliveries or minutes:
///   * 2xx — delivered, terminal.
///   * 408 / 429 / 5xx, or a transport error — retryable; `settle` applies the
///     backoff and `max_attempts` decides when to stop.
///   * any other 4xx — the subscriber is rejecting the request itself and will
///     reject it identically five more times, so fail now.
///   * refused by the outbound-URL guard — never retryable. Re-checked here
///     and not only at registration, because a hostname that resolved to a
///     public address when the webhook was created can resolve to a private one
///     now; that is DNS rebinding, and a registration-time-only guard does
///     nothing about it.
///
/// # Errors
/// Propagates a Postgres claim/settle failure. A delivery failure is recorded,
/// never propagated — one dead endpoint must not stop the queue.
pub async fn drain_once(
    pg_client: &tokio_postgres::Client,
    http: &reqwest::Client,
) -> Result<Option<Delivered>, tokio_postgres::Error> {
    drain_once_with_egress(pg_client, http, crate::urlguard::Egress::Guarded).await
}

/// What one drained attempt did.
#[derive(Debug)]
pub struct Delivered {
    pub delivery_id: String,
    pub webhook_id: String,
    pub state: String,
    pub status: Option<u16>,
    pub error: Option<String>,
}

/// [`drain_once`] with the outbound-URL guard made explicit.
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
/// Propagates a Postgres claim/settle failure.
#[doc(hidden)]
pub async fn drain_once_with_egress(
    pg_client: &tokio_postgres::Client,
    http: &reqwest::Client,
    egress: crate::urlguard::Egress,
) -> Result<Option<Delivered>, tokio_postgres::Error> {
    let Some(delivery) = er_entities::webhook_delivery::update::claim(pg_client)
        .await?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };

    // The subscriber's URL and secret are read at delivery time, so rotating a
    // secret or correcting a URL takes effect on the next attempt rather than
    // being frozen into the queued row.
    let Some((url, secret)) = pg::webhook_endpoint(pg_client, &delivery.webhook_id).await? else {
        let settled = settle(
            pg_client,
            &delivery.id,
            false,
            false,
            None,
            Some("the subscription no longer exists"),
        )
        .await?;
        return Ok(Some(Delivered {
            delivery_id: delivery.id,
            webhook_id: delivery.webhook_id,
            state: settled,
            status: None,
            error: Some("the subscription no longer exists".to_string()),
        }));
    };

    let body = serde_json::to_vec(&delivery.payload).unwrap_or_default();

    if let Err(rejection) = crate::urlguard::check(&url, egress).await {
        let error = format!("refused: {rejection}");
        let settled = settle(pg_client, &delivery.id, false, false, None, Some(&error)).await?;
        return Ok(Some(Delivered {
            delivery_id: delivery.id,
            webhook_id: delivery.webhook_id,
            state: settled,
            status: None,
            error: Some(error),
        }));
    }

    let signature = sign(&secret, &body);
    let (delivered, retryable, status, error) = match post(http, &url, body, &signature).await {
        Ok(code) if (200..300).contains(&code) => (true, false, Some(code), None),
        Ok(code) => {
            let retryable = code == 408 || code == 429 || (500..600).contains(&code);
            (false, retryable, Some(code), Some(format!("HTTP {code}")))
        }
        Err(e) => (false, true, None, Some(e)),
    };
    let settled = settle(
        pg_client,
        &delivery.id,
        delivered,
        retryable,
        status,
        error.as_deref(),
    )
    .await?;
    Ok(Some(Delivered {
        delivery_id: delivery.id,
        webhook_id: delivery.webhook_id,
        state: settled,
        status,
        error,
    }))
}

/// Record the attempt's outcome, returning the resulting state.
async fn settle(
    pg_client: &tokio_postgres::Client,
    delivery_id: &str,
    delivered: bool,
    retryable: bool,
    status: Option<u16>,
    error: Option<&str>,
) -> Result<String, tokio_postgres::Error> {
    let rows = er_entities::webhook_delivery::update::settle(
        pg_client,
        delivered,
        retryable,
        status.map(i64::from),
        error,
        delivery_id,
    )
    .await?;
    Ok(rows
        .into_iter()
        .next()
        .map_or_else(|| "failed".to_string(), |r| r.state))
}

/// Drain until nothing is due, up to `budget` attempts. Returns how many were
/// settled.
///
/// Bounded on purpose: this rides the dispatcher loop, and an unbounded drain
/// would let a large backlog starve job dispatch. Whatever is left is still due
/// on the next pass.
///
/// # Errors
/// Propagates a Postgres claim/settle failure.
pub async fn drain(
    pg_client: &tokio_postgres::Client,
    http: &reqwest::Client,
    budget: usize,
) -> Result<usize, tokio_postgres::Error> {
    let mut settled = 0;
    for _ in 0..budget {
        if drain_once(pg_client, http).await?.is_none() {
            break;
        }
        settled += 1;
    }
    Ok(settled)
}

/// Record the `job.<state>` event for a settled job against the org's
/// subscribers. Delivery happens later, in [`drain_once`].
///
/// # Errors
/// Propagates the Postgres subscriber lookup or insert error.
pub async fn emit_job_event(
    pg_client: &tokio_postgres::Client,
    job: &crate::Job,
) -> Result<Vec<er_entities::webhook_delivery::WebhookDelivery>, tokio_postgres::Error> {
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
    emit(pg_client, &job.org_id, &org_name, &event_type, &data).await
}
