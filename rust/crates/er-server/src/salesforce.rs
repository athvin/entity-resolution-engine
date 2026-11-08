//! A Salesforce Bulk API 2.0 query client, just big enough for the `extract`
//! job: authenticate with the OAuth client-credentials flow, run one SOQL
//! query as a Bulk API 2.0 job, and stream the CSV result pages into one local
//! CSV file in the source's declared column order.
//!
//! Blocking on purpose. The one caller is a pod task body, and pod bodies are
//! synchronous (`dags/tasks.rs`) — `exec_task` runs them on the main thread
//! with no ambient runtime, so `reqwest::blocking` (which drives its own
//! private runtime thread) is the honest shape rather than a hand-rolled
//! `block_on`.
//!
//! The egress discipline is `webhook::client()`'s, restated for the blocking
//! side: redirects are refused outright, because a redirect is how a vetted
//! URL turns into an unvetted one after the guard has looked. The outbound-URL
//! guard itself runs where the DNS answer is fresh — at registration
//! (`create_salesforce_connection`) and again when the work order resolves the
//! credential (`attempt_work_order`) — not here; this module is transport.
//!
//! What this module must never do: log, record on a span, or interpolate into
//! an error the client secret or the access token. Errors carry Salesforce's
//! own messages and status codes, which never echo the credential.

use std::time::Duration;

/// How long one HTTP exchange may take. Result pages are bounded by
/// [`MAX_RECORDS_PER_PAGE`], so a stalled read is a stuck connection rather
/// than a big page, and five minutes is generous for either.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// Rows per result page. Salesforce caps pages by its own limits too; naming
/// one here bounds a page's memory and makes the per-request timeout mean
/// something.
const MAX_RECORDS_PER_PAGE: u32 = 50_000;

/// The poll schedule for a running query job: 2s doubling to a 30s ceiling,
/// for at most an hour. A query still running after an hour is not going to
/// be saved by more polling, and the task's own policy timeout is the real
/// backstop.
const POLL_START: Duration = Duration::from_secs(2);
const POLL_CAP: Duration = Duration::from_secs(30);
const POLL_BUDGET: Duration = Duration::from_secs(3600);

/// A resolved connection, as the work order carries it — the credential half
/// of `salesforce_connections`, never persisted here.
pub struct Connection {
    pub instance_url: String,
    pub client_id: String,
    pub client_secret: String,
    /// The REST version segment, e.g. `v62.0`.
    pub api_version: String,
}

/// A transport failure, pre-sorted into the engine's retry taxonomy so the
/// task body maps variants to error classes instead of re-reading messages.
#[derive(Debug)]
pub enum SfError {
    /// The credential was refused — retrying cannot help until it changes.
    Auth(String),
    /// Salesforce rejected the query itself (unknown object, bad field, bad
    /// SOQL) — a precondition, fixed by changing the request.
    Refused(String),
    /// Network trouble, timeouts, 5xx — worth a retry.
    Transient(String),
    /// A response this client cannot read; neither a retry nor the caller can
    /// fix it, so it surfaces as the unclassified default.
    Protocol(String),
}

impl std::fmt::Display for SfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SfError::Auth(m) => write!(f, "salesforce auth: {m}"),
            SfError::Refused(m) => write!(f, "salesforce refused the query: {m}"),
            SfError::Transient(m) => write!(f, "salesforce request: {m}"),
            SfError::Protocol(m) => write!(f, "salesforce protocol: {m}"),
        }
    }
}

/// What one extract pulled.
#[derive(Debug)]
pub struct ExtractOutcome {
    pub rows: u64,
    /// The max value of the updated-at column among extracted rows, verbatim —
    /// the next incremental run's watermark. `None` when no row carried one.
    pub max_updated: Option<String>,
}

/// The blocking HTTP client every call in this module uses: redirects refused
/// (the `webhook::client()` rule), both timeouts set.
fn http() -> Result<reqwest::blocking::Client, SfError> {
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| SfError::Protocol(format!("building the http client: {e}")))
}

/// A SOQL identifier: an sObject or field API name. The charset is what
/// Salesforce itself allows (`Account`, `SystemModstamp`, `My_Field__c`), and
/// refusing anything else is what keeps a config-supplied column name from
/// smuggling SOQL into the query this module assembles.
fn valid_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A SOQL datetime literal, e.g. `2026-11-08T12:34:56.000Z`. The watermark
/// is data READ FROM the tenant's Salesforce org, so it is validated like any
/// other untrusted input before it is placed into a query — a crafted
/// `SystemModstamp` must not become SOQL.
fn valid_datetime_literal(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() < 20 || bytes.len() > 29 {
        return false;
    }
    let date_time_ok = bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':';
    date_time_ok
        && s.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '-' | 'T' | ':' | '.' | '+' | 'Z'))
}

/// Assemble the extract's SOQL. Every structural piece — object, columns, the
/// updated-at column, the watermark — is validated against the shape it must
/// have; the tenant's own `where` clause rides through verbatim, because it is
/// steward-supplied SOQL against the steward's own org, the same trust the
/// query itself gets.
///
/// # Errors
/// A message starting `param:` (the shape `engine_error_class` reads as a
/// precondition) naming what was malformed.
pub fn build_soql(
    object: &str,
    columns: &[String],
    updated_at_column: &str,
    watermark: Option<&str>,
    where_clause: Option<&str>,
    exclude_converted_leads: bool,
) -> Result<String, String> {
    if !valid_identifier(object) {
        return Err(format!("param: object {object:?} is not an sObject API name"));
    }
    if columns.is_empty() {
        return Err("param: the source declares no columns to extract".to_string());
    }
    for c in columns {
        if !valid_identifier(c) {
            return Err(format!("param: column {c:?} is not a field API name"));
        }
    }
    let mut conjuncts: Vec<String> = Vec::new();
    if let Some(mark) = watermark {
        if !valid_identifier(updated_at_column) {
            return Err(format!(
                "param: updated-at column {updated_at_column:?} is not a field API name"
            ));
        }
        if !valid_datetime_literal(mark) {
            return Err(format!(
                "param: watermark {mark:?} is not a SOQL datetime literal"
            ));
        }
        conjuncts.push(format!("{updated_at_column} > {mark}"));
    }
    if exclude_converted_leads {
        conjuncts.push("IsConverted = false".to_string());
    }
    if let Some(w) = where_clause.map(str::trim).filter(|w| !w.is_empty()) {
        conjuncts.push(format!("({w})"));
    }
    let mut soql = format!("SELECT {} FROM {object}", columns.join(", "));
    if !conjuncts.is_empty() {
        soql.push_str(" WHERE ");
        soql.push_str(&conjuncts.join(" AND "));
    }
    Ok(soql)
}

/// One decoded error from a Salesforce JSON error body, or the raw text when
/// the body is not the documented shape.
fn api_error_text(status: u16, body: &str) -> String {
    let parsed: Option<String> = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            let one = |e: &serde_json::Value| {
                let code = e
                    .get("errorCode")
                    .or_else(|| e.get("error"))
                    .and_then(|c| c.as_str())
                    .unwrap_or("error");
                let message = e
                    .get("message")
                    .or_else(|| e.get("error_description"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("");
                format!("{code}: {message}")
            };
            match &v {
                serde_json::Value::Array(items) => items.first().map(one),
                serde_json::Value::Object(_) => Some(one(&v)),
                _ => None,
            }
        });
    let text = parsed.unwrap_or_else(|| {
        let mut t = body.trim().to_string();
        t.truncate(300);
        t
    });
    format!("HTTP {status} — {text}")
}

/// Exchange the client-credentials pair for an access token.
///
/// The token endpoint lives on the connection's own My Domain
/// (`{instance}/services/oauth2/token`) — the client-credentials flow requires
/// that host, which is also what keeps this module single-origin.
fn token(http: &reqwest::blocking::Client, conn: &Connection) -> Result<String, SfError> {
    let url = format!("{}/services/oauth2/token", conn.instance_url);
    let resp = http
        .post(&url)
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", conn.client_id.as_str()),
            ("client_secret", conn.client_secret.as_str()),
        ])
        .send()
        .map_err(|e| SfError::Transient(format!("token request: {e}")))?;
    let status = resp.status().as_u16();
    let body = resp
        .text()
        .map_err(|e| SfError::Transient(format!("token response: {e}")))?;
    if status != 200 {
        // 400/401 here is the credential being wrong — config, not transient.
        let detail = api_error_text(status, &body);
        return if status >= 500 {
            Err(SfError::Transient(detail))
        } else {
            Err(SfError::Auth(detail))
        };
    }
    serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("access_token").and_then(|t| t.as_str()).map(String::from))
        .ok_or_else(|| SfError::Protocol("the token response carries no access_token".to_string()))
}

/// Run `soql` as a Bulk API 2.0 query job and append every result row to
/// `writer` in `columns` order; the header row is written by the CALLER, so a
/// retry that truncates and rewrites the file owns the whole layout.
///
/// Returns the row count and the max updated-at value seen — the next
/// incremental extract's watermark.
///
/// # Errors
/// Pre-sorted by [`SfError`]: an auth refusal is config, a rejected query is a
/// precondition, network trouble is transient.
pub fn extract_to_writer<W: std::io::Write>(
    conn: &Connection,
    soql: &str,
    columns: &[String],
    updated_at_column: &str,
    writer: &mut W,
) -> Result<ExtractOutcome, SfError> {
    let http = http()?;
    let bearer = token(&http, conn)?;
    let base = format!(
        "{}/services/data/{}/jobs/query",
        conn.instance_url, conn.api_version
    );

    // Create the query job. `lineEnding: LF` is pinned so the result bytes
    // never depend on an org-level default.
    let create = http
        .post(&base)
        .bearer_auth(&bearer)
        .json(&serde_json::json!({
            "operation": "query",
            "query": soql,
            "contentType": "CSV",
            "columnDelimiter": "COMMA",
            "lineEnding": "LF",
        }))
        .send()
        .map_err(|e| SfError::Transient(format!("creating the query job: {e}")))?;
    let status = create.status().as_u16();
    let body = create
        .text()
        .map_err(|e| SfError::Transient(format!("query-job response: {e}")))?;
    if status >= 500 {
        return Err(SfError::Transient(api_error_text(status, &body)));
    }
    if status != 200 {
        return Err(SfError::Refused(api_error_text(status, &body)));
    }
    let job_id = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(String::from))
        .ok_or_else(|| SfError::Protocol("the query-job response carries no id".to_string()))?;

    // Poll until the job settles. `Failed`/`Aborted` carry the reason on the
    // job resource, and an unknown state is a protocol error rather than an
    // infinite loop.
    let mut waited = Duration::ZERO;
    let mut delay = POLL_START;
    loop {
        let resp = http
            .get(format!("{base}/{job_id}"))
            .bearer_auth(&bearer)
            .send()
            .map_err(|e| SfError::Transient(format!("polling the query job: {e}")))?;
        let status = resp.status().as_u16();
        let body = resp
            .text()
            .map_err(|e| SfError::Transient(format!("poll response: {e}")))?;
        if status != 200 {
            return Err(SfError::Transient(api_error_text(status, &body)));
        }
        let v: serde_json::Value = serde_json::from_str(&body)
            .map_err(|_| SfError::Protocol("the job status is not JSON".to_string()))?;
        match v.get("state").and_then(|s| s.as_str()).unwrap_or("") {
            "JobComplete" => break,
            "UploadComplete" | "InProgress" => {
                if waited >= POLL_BUDGET {
                    return Err(SfError::Transient(format!(
                        "the query job is still running after {}s",
                        POLL_BUDGET.as_secs()
                    )));
                }
                std::thread::sleep(delay);
                waited += delay;
                delay = (delay * 2).min(POLL_CAP);
            }
            "Failed" | "Aborted" => {
                let why = v
                    .get("errorMessage")
                    .and_then(|m| m.as_str())
                    .unwrap_or("no errorMessage");
                return Err(SfError::Refused(format!("job {job_id}: {why}")));
            }
            other => {
                return Err(SfError::Protocol(format!("unknown job state {other:?}")));
            }
        }
    }

    // Page the results. Every page repeats the CSV header, so each page is
    // parsed with its own header and rows are re-emitted in OUR column order —
    // which also makes the output independent of any field-order choice
    // Salesforce makes.
    let mut rows: u64 = 0;
    let mut max_updated: Option<String> = None;
    let mut locator: Option<String> = None;
    let mut out = csv::Writer::from_writer(writer);
    loop {
        let mut req = http
            .get(format!("{base}/{job_id}/results"))
            .bearer_auth(&bearer)
            .query(&[("maxRecords", MAX_RECORDS_PER_PAGE.to_string())]);
        if let Some(l) = &locator {
            req = req.query(&[("locator", l.as_str())]);
        }
        let resp = req
            .send()
            .map_err(|e| SfError::Transient(format!("fetching results: {e}")))?;
        let status = resp.status().as_u16();
        let next = resp
            .headers()
            .get("Sforce-Locator")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body = resp
            .bytes()
            .map_err(|e| SfError::Transient(format!("reading a results page: {e}")))?;
        if status != 200 {
            return Err(SfError::Transient(api_error_text(
                status,
                &String::from_utf8_lossy(&body),
            )));
        }

        let mut page = csv::Reader::from_reader(body.as_ref());
        let headers = page
            .headers()
            .map_err(|e| SfError::Protocol(format!("results page header: {e}")))?
            .clone();
        // Field API names match case-insensitively: Salesforce echoes the
        // field's own casing, not the query's.
        let index_of = |name: &str| {
            headers
                .iter()
                .position(|h| h.eq_ignore_ascii_case(name))
        };
        let order: Vec<Option<usize>> = columns.iter().map(|c| index_of(c)).collect();
        let updated_idx = index_of(updated_at_column);
        for record in page.records() {
            let record =
                record.map_err(|e| SfError::Protocol(format!("results page row: {e}")))?;
            let row: Vec<&str> = order
                .iter()
                .map(|idx| idx.and_then(|i| record.get(i)).unwrap_or(""))
                .collect();
            out.write_record(&row)
                .map_err(|e| SfError::Protocol(format!("writing a row: {e}")))?;
            rows += 1;
            if let Some(stamp) = updated_idx.and_then(|i| record.get(i))
                && valid_datetime_literal(stamp)
                && max_updated.as_deref().is_none_or(|m| stamp > m)
            {
                max_updated = Some(stamp.to_string());
            }
        }

        match next.as_deref() {
            None | Some("" | "null") => break,
            Some(l) => locator = Some(l.to_string()),
        }
    }
    out.flush()
        .map_err(|e| SfError::Protocol(format!("flushing the output: {e}")))?;
    Ok(ExtractOutcome { rows, max_updated })
}

/// Write the header row for `columns` — split from [`extract_to_writer`] so
/// the caller truncates-and-rewrites atomically on retry.
///
/// # Errors
/// The underlying write.
pub fn write_header<W: std::io::Write>(columns: &[String], writer: &mut W) -> std::io::Result<()> {
    let mut line = columns.join(",");
    line.push('\n');
    writer.write_all(line.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soql_assembles_each_clause_exactly_once() {
        let cols = vec!["Id".to_string(), "Email".to_string()];
        let soql = build_soql(
            "Lead",
            &cols,
            "SystemModstamp",
            Some("2026-01-02T03:04:05.000Z"),
            Some("Country = 'US'"),
            true,
        )
        .unwrap();
        assert_eq!(
            soql,
            "SELECT Id, Email FROM Lead WHERE SystemModstamp > 2026-01-02T03:04:05.000Z \
             AND IsConverted = false AND (Country = 'US')"
        );
        // The bare form has no WHERE at all — a trailing `WHERE` is invalid SOQL.
        assert_eq!(
            build_soql("Contact", &cols, "SystemModstamp", None, None, false).unwrap(),
            "SELECT Id, Email FROM Contact"
        );
    }

    /// The structural pieces are validated as identifiers, because the object
    /// and columns come from job params and config — tenant input — and the
    /// watermark comes from Salesforce data itself. None of them may smuggle
    /// SOQL into the query.
    #[test]
    fn injection_shaped_inputs_are_refused_as_params() {
        let cols = vec!["Id".to_string()];
        for (object, columns, mark) in [
            ("Lead WHERE 1=1", vec!["Id".to_string()], None),
            ("Lead", vec!["Id, Name FROM Account".to_string()], None),
            (
                "Lead",
                vec!["Id".to_string()],
                Some("2026-01-01T00:00:00Z OR Name != null"),
            ),
        ] {
            let err = build_soql(object, &columns, "SystemModstamp", mark, None, false)
                .expect_err("refused");
            assert!(err.starts_with("param:"), "{err}");
        }
        let _ = cols;
    }

    #[test]
    fn a_custom_object_and_field_are_identifiers() {
        let cols = vec!["Id".to_string(), "Legacy_Id__c".to_string()];
        let soql =
            build_soql("Invoice__c", &cols, "SystemModstamp", None, None, false).unwrap();
        assert_eq!(soql, "SELECT Id, Legacy_Id__c FROM Invoice__c");
    }

    #[test]
    fn datetime_literals_accept_salesforce_shapes_only() {
        for good in [
            "2026-11-08T12:34:56Z",
            "2026-11-08T12:34:56.000Z",
            "2026-11-08T12:34:56.000+0000",
        ] {
            assert!(valid_datetime_literal(good), "{good}");
        }
        for bad in ["", "yesterday", "2026-11-08", "2026-11-08T12:34:56.000Z OR 1=1"] {
            assert!(!valid_datetime_literal(bad), "{bad}");
        }
    }
}
