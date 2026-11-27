//! The Salesforce calls a merge apply makes (#995): read members' live
//! revision, update the master, merge victims into it. Acts with a delegated
//! access token ([`crate::sf_oauth::access_token`]), never client credentials.
//!
//! Every outcome is classified, because the apply's safety depends on it:
//!
//! - a READ is safe to retry, so 429/5xx/network failures are retried with
//!   bounded backoff;
//! - a MUTATION is retried only on a response that proves it was not
//!   processed (429, 503). Any other failure after the request may have
//!   reached Salesforce — a timeout, a reset, a 500 — is
//!   [`CallError::Ambiguous`], and the caller resolves it by re-reading
//!   rather than replaying.
//!
//! Errors carry Salesforce's error code, never a body, record value or token.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::urlguard::{self, Egress};

/// At most this many attempts per call.
const MAX_ATTEMPTS: u32 = 3;
/// A `Retry-After` longer than this is not waited out inside a job: the call
/// fails as rate-limited and the apply resumes on a later run.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);
/// Salesforce's `merge()` takes one master and at most two victims.
pub const MERGE_BATCH: usize = 2;

/// Who to call, as whom.
#[derive(Clone)]
pub struct Org {
    pub instance_url: String,
    pub access_token: String,
    /// `v62.0` style, from the connection.
    pub api_version: String,
    pub egress: Egress,
    /// Backoff base between retries; production uses one second.
    pub backoff: Duration,
}

/// One member as Salesforce has it now. A merged-away record is deleted with
/// `MasterRecordId` naming the survivor — the evidence an external merge
/// leaves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Live {
    pub system_modstamp: String,
    pub is_deleted: bool,
    pub master_record_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CallError {
    /// Rate limited and the retries (or the `Retry-After`) ran out. The call
    /// was not processed.
    #[error("Salesforce rate-limited the call ({0})")]
    RateLimited(String),
    /// The org's API request budget is exhausted. Not processed.
    #[error("the Salesforce API request limit is exhausted")]
    BudgetExhausted,
    /// The request may or may not have been processed.
    #[error("the outcome of the Salesforce call is unknown ({0})")]
    Ambiguous(String),
    /// Salesforce refused it, definitively; carries the error code.
    #[error("Salesforce refused the call ({0})")]
    Rejected(String),
    /// A read failed in a way worth trying again later.
    #[error("Salesforce is unreachable ({0})")]
    Transient(String),
    #[error("the destination is refused")]
    Refused(#[from] urlguard::UrlRejection),
}

/// A Salesforce record id: 15 or 18 alphanumerics. Checked before an id is
/// put in SOQL or XML.
#[must_use]
pub fn valid_id(id: &str) -> bool {
    matches!(id.len(), 15 | 18) && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn client() -> Result<reqwest::Client, CallError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| CallError::Transient(format!("client: {e}")))
}

fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// The first `errorCode` of a REST error array, or the SOAP fault code.
fn error_code(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body)
        && let Some(code) = v
            .get(0)
            .and_then(|e| e.get("errorCode"))
            .and_then(|c| c.as_str())
    {
        return code.to_string();
    }
    for tag in ["faultcode", "statusCode"] {
        if let Some(code) = between(body, &format!("<{tag}>"), &format!("</{tag}>")) {
            return code.trim_start_matches("sf:").to_string();
        }
    }
    "unknown".to_string()
}

fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = s.find(open)? + open.len();
    let end = s[start..].find(close)? + start;
    Some(&s[start..end])
}

/// What one HTTP exchange produced.
enum Attempt {
    Done(String),
    /// Not processed; wait this long (or the backoff) and try again.
    Retry(String, Option<Duration>),
    Fail(CallError),
}

fn classify(
    status: u16,
    headers: &reqwest::header::HeaderMap,
    body: String,
    mutation: bool,
) -> Attempt {
    if (200..300).contains(&status) {
        return Attempt::Done(body);
    }
    let code = error_code(&body);
    if code == "REQUEST_LIMIT_EXCEEDED" {
        return Attempt::Fail(CallError::BudgetExhausted);
    }
    match status {
        429 | 503 => Attempt::Retry(format!("http {status}"), retry_after(headers)),
        s if s >= 500 && mutation => Attempt::Fail(CallError::Ambiguous(format!("http {s}"))),
        s if s >= 500 => Attempt::Retry(format!("http {s}"), None),
        _ => Attempt::Fail(CallError::Rejected(code)),
    }
}

async fn call(
    org: &Org,
    build: impl Fn(&reqwest::Client) -> reqwest::RequestBuilder,
    url: &str,
    mutation: bool,
) -> Result<String, CallError> {
    urlguard::check(url, org.egress).await?;
    let http = client()?;
    let mut last = String::new();
    for attempt in 0..MAX_ATTEMPTS {
        let outcome = match build(&http).send().await {
            Err(e) if mutation && !e.is_connect() => Attempt::Fail(CallError::Ambiguous(format!(
                "request: {}",
                e.without_url()
            ))),
            // Not connected means never sent: safe to retry even a mutation.
            Err(e) => Attempt::Retry(format!("request: {}", e.without_url()), None),
            Ok(resp) => {
                let status = resp.status().as_u16();
                let headers = resp.headers().clone();
                match resp.text().await {
                    Ok(body) => classify(status, &headers, body, mutation),
                    Err(e) if mutation => Attempt::Fail(CallError::Ambiguous(format!(
                        "response: {}",
                        e.without_url()
                    ))),
                    Err(e) => Attempt::Retry(format!("response: {}", e.without_url()), None),
                }
            }
        };
        match outcome {
            Attempt::Done(body) => return Ok(body),
            Attempt::Fail(e) => return Err(e),
            Attempt::Retry(why, wait) => {
                if wait.is_some_and(|w| w > MAX_RETRY_AFTER) {
                    return Err(CallError::RateLimited(why));
                }
                last = why;
                if attempt + 1 < MAX_ATTEMPTS {
                    tokio::time::sleep(wait.unwrap_or(org.backoff * 2u32.pow(attempt))).await;
                }
            }
        }
    }
    if last.starts_with("http 429") || last.starts_with("http 503") {
        Err(CallError::RateLimited(last))
    } else {
        Err(CallError::Transient(last))
    }
}

fn base(org: &Org) -> String {
    org.instance_url.trim_end_matches('/').to_string()
}

/// Every member's live state, including merged-away ones (`queryAll` sees
/// deleted rows). An id absent from the map does not exist at all.
///
/// # Errors
/// See [`CallError`].
pub async fn live_state(
    org: &Org,
    object: &str,
    ids: &[String],
) -> Result<BTreeMap<String, Live>, CallError> {
    if ids.iter().any(|id| !valid_id(id)) {
        return Err(CallError::Rejected("INVALID_ID".into()));
    }
    let quoted: Vec<String> = ids.iter().map(|id| format!("'{id}'")).collect();
    let soql = format!(
        "SELECT Id, SystemModstamp, IsDeleted, MasterRecordId FROM {object} WHERE Id IN ({})",
        quoted.join(",")
    );
    let url = format!("{}/services/data/{}/queryAll", base(org), org.api_version);
    let body = call(
        org,
        |h| {
            h.get(&url)
                .bearer_auth(&org.access_token)
                .query(&[("q", soql.as_str())])
        },
        &url,
        false,
    )
    .await?;
    let v: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| CallError::Transient("query response is not JSON".into()))?;
    let mut out = BTreeMap::new();
    for r in v
        .get("records")
        .and_then(|r| r.as_array())
        .into_iter()
        .flatten()
    {
        let s = |k: &str| r.get(k).and_then(|x| x.as_str()).map(str::to_string);
        let Some(id) = s("Id") else { continue };
        // A 15-character id in the snapshot matches the 18-character form's prefix.
        let key = ids
            .iter()
            .find(|want| id.starts_with(want.as_str()) || want.starts_with(&id))
            .cloned()
            .unwrap_or(id);
        out.insert(
            key,
            Live {
                system_modstamp: s("SystemModstamp").unwrap_or_default(),
                is_deleted: r
                    .get("IsDeleted")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                master_record_id: s("MasterRecordId"),
            },
        );
    }
    Ok(out)
}

/// Write the approved field values to the master.
///
/// # Errors
/// See [`CallError`]; `Ambiguous` means re-read before deciding.
pub async fn update(
    org: &Org,
    object: &str,
    id: &str,
    fields: &BTreeMap<String, serde_json::Value>,
) -> Result<(), CallError> {
    if !valid_id(id) {
        return Err(CallError::Rejected("INVALID_ID".into()));
    }
    let url = format!(
        "{}/services/data/{}/sobjects/{object}/{id}",
        base(org),
        org.api_version
    );
    call(
        org,
        |h| h.patch(&url).bearer_auth(&org.access_token).json(fields),
        &url,
        true,
    )
    .await
    .map(|_| ())
}

/// Merge up to [`MERGE_BATCH`] victims into the master (SOAP `merge()`; the
/// REST API has no merge). Salesforce reparents related records natively.
///
/// # Errors
/// See [`CallError`]; `Ambiguous` means re-read before deciding.
pub async fn merge(
    org: &Org,
    object: &str,
    master: &str,
    victims: &[String],
) -> Result<(), CallError> {
    if victims.is_empty()
        || victims.len() > MERGE_BATCH
        || !valid_id(master)
        || victims.iter().any(|v| !valid_id(v))
    {
        return Err(CallError::Rejected("INVALID_ID".into()));
    }
    let version = org.api_version.trim_start_matches('v');
    let url = format!("{}/services/Soap/u/{version}", base(org));
    let merge_ids: String = victims.iter().fold(String::new(), |mut out, v| {
        use std::fmt::Write as _;
        let _ = write!(out, "<urn:recordToMergeIds>{v}</urn:recordToMergeIds>");
        out
    });
    let envelope = format!(
        concat!(
            r#"<?xml version="1.0" encoding="utf-8"?>"#,
            r#"<soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/" "#,
            r#"xmlns:urn="urn:partner.soap.sforce.com" xmlns:urn1="urn:sobject.partner.soap.sforce.com" "#,
            r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">"#,
            "<soapenv:Header><urn:SessionHeader><urn:sessionId>{token}</urn:sessionId></urn:SessionHeader></soapenv:Header>",
            "<soapenv:Body><urn:merge><urn:request>",
            r#"<urn:masterRecord xsi:type="urn1:sObject"><urn1:type>{object}</urn1:type><urn1:Id>{master}</urn1:Id></urn:masterRecord>"#,
            "{ids}</urn:request></urn:merge></soapenv:Body></soapenv:Envelope>"
        ),
        token = xml_escape(&org.access_token),
        object = object,
        master = master,
        ids = merge_ids,
    );
    let body = call(
        org,
        |h| {
            h.post(&url)
                .header(reqwest::header::CONTENT_TYPE, "text/xml; charset=utf-8")
                .header("SOAPAction", "merge")
                .body(envelope.clone())
        },
        &url,
        true,
    )
    .await?;
    if between(&body, "<success>", "</success>") == Some("true") {
        Ok(())
    } else {
        Err(CallError::Rejected(error_code(&body)))
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_checked_before_reaching_soql_or_xml() {
        assert!(valid_id("003000000000001"));
        assert!(valid_id("003000000000001AAA"));
        assert!(!valid_id("003' OR Id != '"));
        assert!(!valid_id("short"));
    }

    #[test]
    fn mutations_are_ambiguous_on_5xx_but_retry_on_429() {
        let h = reqwest::header::HeaderMap::new();
        assert!(matches!(
            classify(500, &h, String::new(), true),
            Attempt::Fail(CallError::Ambiguous(_))
        ));
        assert!(matches!(
            classify(500, &h, String::new(), false),
            Attempt::Retry(..)
        ));
        assert!(matches!(
            classify(429, &h, String::new(), true),
            Attempt::Retry(..)
        ));
        let limit = r#"[{"errorCode":"REQUEST_LIMIT_EXCEEDED","message":"x"}]"#.to_string();
        assert!(matches!(
            classify(403, &h, limit, true),
            Attempt::Fail(CallError::BudgetExhausted)
        ));
        let fault = "<faultcode>sf:INVALID_SESSION_ID</faultcode>".to_string();
        assert!(matches!(
            classify(500, &h, fault.clone(), false),
            Attempt::Retry(..)
        ));
        assert_eq!(error_code(&fault), "INVALID_SESSION_ID");
    }
}
