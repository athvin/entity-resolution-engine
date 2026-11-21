//! Synchronous execution for the server's bounded blocking lake-read pool.
//! Remote endpoints and credentials come only from an operator-resolved binding.
//! No request is replayed, redirected or silently moved to another session.

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use arrow::ipc::reader::StreamReader;
use arrow::record_batch::RecordBatch;
use er_entities::execution::{Executor, Parameter};
use reqwest::Method;
use reqwest::blocking::Client;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::Binding;

pub mod recovery;
mod task_options;
pub use task_options::{Compute, TaskOptions};

/// Write-ahead evidence sink for a remote task session. Each call must be
/// durable before it returns `Ok`; a refusal means the identifier was not
/// recorded, so the corresponding SQL is never sent. Implementations store
/// identifiers only — never SQL, bind values or credentials.
pub trait Journal: Send + Sync {
    /// The acknowledged remote session, before any SQL is submitted.
    ///
    /// # Errors
    /// Refusal (a fenced owner, an unreachable control plane) blocks all SQL.
    fn session(&self, id: Uuid) -> Result<(), JournalRefused>;
    /// The next request id, before the request is sent. `commit` marks the
    /// request whose success publishes the transaction.
    ///
    /// # Errors
    /// Refusal blocks this and every later request on the session.
    fn request(&self, id: Uuid, commit: bool) -> Result<(), JournalRefused>;
}

/// The journal could not durably record an identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("execution journal refused")]
pub struct JournalRefused;

/// What the owning process knows about the publication request it sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitState {
    /// No COMMIT was journaled, so none was sent: nothing can be published.
    NotSent,
    /// A COMMIT was journaled and may have reached the service.
    Sent,
    /// The COMMIT returned a statement result: the transaction is published.
    Returned,
}

const MAX_RESULT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_CONTROL_BYTES: u64 = 64 * 1024;
const STATEMENT_TIMEOUT: Duration = Duration::from_secs(75);

/// A bounded classification, never a server body, SQL text, bind or credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Failure {
    #[error("execution session is closed")]
    Closed,
    #[error("execution session was lost")]
    SessionLost,
    #[error("execution outcome is unknown")]
    UnknownOutcome,
    #[error("execution transport unavailable")]
    Transport,
    #[error("invalid execution response")]
    Protocol,
    #[error("execution limits exceeded")]
    Limit,
    #[error("unsupported execution parameter")]
    Parameter,
    #[error("execution authorization refused")]
    Unauthorized,
    #[error("execution capacity unavailable")]
    Capacity,
    #[error("execution query failed")]
    Query,
    #[error("execution query cancelled")]
    Cancelled,
    #[error("execution journal refused")]
    Journal,
}

/// A request identifier can be reconciled by the operator after transport loss.
/// It is diagnostic identity, never permission to replay a query.
#[derive(Debug, thiserror::Error)]
#[error("{kind}")]
pub struct ExecutionError {
    pub kind: Failure,
    pub request_id: Option<Uuid>,
}

impl From<Failure> for ExecutionError {
    fn from(kind: Failure) -> Self {
        Self {
            kind,
            request_id: None,
        }
    }
}

#[derive(Clone)]
struct Transport {
    endpoint: String,
    token: String,
    http: Client,
}

impl Transport {
    fn new(endpoint: String, token: String) -> Result<Self, ExecutionError> {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| Failure::Transport)?;
        Ok(Self {
            endpoint,
            token,
            http,
        })
    }

    fn request(
        &self,
        method: Method,
        path: &str,
        payload: Option<&Value>,
        timeout: Duration,
        arrow: bool,
    ) -> Result<Vec<u8>, ExecutionError> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.endpoint))
            .bearer_auth(&self.token)
            .timeout(timeout);
        if let Some(payload) = payload {
            let body = serde_json::to_vec(payload).map_err(|_| Failure::Parameter)?;
            request = request
                .header("Content-Type", "application/json")
                .body(body);
        }
        let response = request.send().map_err(|_| Failure::Transport)?;
        let status = response.status();
        let maximum = if arrow && status.is_success() {
            MAX_RESULT_BYTES
        } else {
            MAX_CONTROL_BYTES
        };
        if response
            .content_length()
            .is_some_and(|length| length > maximum)
        {
            return Err(Failure::Limit.into());
        }
        let mut bytes = Vec::new();
        response
            .take(maximum + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::Transport)?;
        if bytes.len() as u64 > maximum {
            return Err(Failure::Limit.into());
        }
        if status.is_success() {
            return Ok(bytes);
        }
        let payload: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let kind = match (
            status.as_u16(),
            payload.get("failure_kind").and_then(Value::as_str),
        ) {
            (401 | 403, _) => Failure::Unauthorized,
            (_, Some("unknown_session" | "session_lost" | "worker_lost")) => Failure::SessionLost,
            (_, Some("canceled" | "cancelled" | "query_cancelled")) => Failure::Cancelled,
            (_, Some("timeout" | "interrupted" | "budget_exceeded")) => Failure::UnknownOutcome,
            (413, _) => Failure::Limit,
            (_, Some("capacity_unavailable" | "capacity" | "compute_unavailable")) => {
                Failure::Capacity
            }
            (400, Some("sql_error")) => Failure::Query,
            _ => Failure::Protocol,
        };
        Err(kind.into())
    }
}

/// One owned execution session. Opening the embedded session is deliberately
/// separate from attaching an authorized lake, just as it is for a remote
/// session. Persistent SQL still comes from generated statements.
pub enum Session {
    Embedded(Option<duckdb::Connection>),
    Remote(RemoteSession),
}

impl Session {
    pub fn open_embedded() -> Result<Self, ExecutionError> {
        duckdb::Connection::open_in_memory()
            .map(|connection| Self::Embedded(Some(connection)))
            .map_err(|_| Failure::Query.into())
    }

    /// Adopt an already configured embedded connection without changing it.
    pub fn embedded(connection: duckdb::Connection) -> Self {
        Self::Embedded(Some(connection))
    }

    /// Disposable integration harness only; never compiled into the product.
    #[cfg(feature = "disposable-proof")]
    pub fn open_loopback_proof(
        address: std::net::SocketAddr,
        token: String,
    ) -> Result<Self, ExecutionError> {
        if !address.ip().is_loopback() {
            return Err(Failure::Unauthorized.into());
        }
        RemoteSession::open(Transport::new(format!("http://{address}"), token)?).map(Self::Remote)
    }

    pub fn open_remote(binding: &Binding<'_>) -> Result<Self, ExecutionError> {
        RemoteSession::open(Transport::new(
            binding.endpoint(),
            binding.bearer_token().to_owned(),
        )?)
        .map(Self::Remote)
    }

    /// Open a task session with explicit placement and bounded execution policy.
    pub fn open_remote_task(
        binding: &Binding<'_>,
        options: TaskOptions,
    ) -> Result<Self, ExecutionError> {
        RemoteSession::open_task(
            Transport::new(binding.endpoint(), binding.bearer_token().to_owned())?,
            options,
            None,
        )
        .map(Self::Remote)
    }

    /// As `open_remote_task`, journaling the session and every request id
    /// before it takes effect. A journal refusal sends no further SQL.
    pub fn open_remote_task_journaled(
        binding: &Binding<'_>,
        options: TaskOptions,
        journal: Arc<dyn Journal>,
    ) -> Result<Self, ExecutionError> {
        RemoteSession::open_task(
            Transport::new(binding.endpoint(), binding.bearer_token().to_owned())?,
            options,
            Some(journal),
        )
        .map(Self::Remote)
    }

    /// The publication request's state. Embedded sessions commit locally and
    /// never need remote reconciliation, so they report `NotSent`.
    pub fn commit_state(&self) -> CommitState {
        match self {
            Self::Embedded(_) => CommitState::NotSent,
            Self::Remote(session) => session.commit_state(),
        }
    }

    /// Whether any request was journaled and handed to the transport.
    pub fn request_sent(&self) -> bool {
        match self {
            Self::Embedded(_) => false,
            Self::Remote(session) => session.request_sent.load(Ordering::Acquire),
        }
    }

    /// Actual-service proof entry point, restricted to loopback and test builds.
    #[cfg(feature = "disposable-proof")]
    pub fn open_loopback_task_proof(
        address: std::net::SocketAddr,
        token: String,
        options: TaskOptions,
    ) -> Result<Self, ExecutionError> {
        if !address.ip().is_loopback() {
            return Err(Failure::Unauthorized.into());
        }
        RemoteSession::open_task(
            Transport::new(format!("http://{address}"), token)?,
            options,
            None,
        )
        .map(Self::Remote)
    }

    /// Journaled loopback proof entry point, restricted to test builds.
    #[cfg(feature = "disposable-proof")]
    pub fn open_loopback_task_proof_journaled(
        address: std::net::SocketAddr,
        token: String,
        options: TaskOptions,
        journal: Arc<dyn Journal>,
    ) -> Result<Self, ExecutionError> {
        if !address.ip().is_loopback() {
            return Err(Failure::Unauthorized.into());
        }
        RemoteSession::open_task(
            Transport::new(format!("http://{address}"), token)?,
            options,
            Some(journal),
        )
        .map(Self::Remote)
    }

    pub fn cancellation(&self) -> Result<Cancellation, ExecutionError> {
        match self {
            Self::Embedded(Some(connection)) => {
                Ok(Cancellation::Embedded(connection.interrupt_handle()))
            }
            Self::Embedded(None) => Err(Failure::Closed.into()),
            Self::Remote(session) => Ok(Cancellation::Remote(RemoteCancellation {
                transport: session.transport.clone(),
                path: session.path.clone(),
                closed: session.closed.clone(),
            })),
        }
    }

    /// Explicit close reports cleanup failure. Dropping an embedded connection
    /// rolls back an open transaction. Remote Drop is only a bounded best effort;
    /// callers must use this method to observe and handle cleanup failure.
    pub fn close(&mut self) -> Result<(), ExecutionError> {
        match self {
            Self::Embedded(connection) => {
                drop(connection.take());
                Ok(())
            }
            Self::Remote(session) => session.close(STATEMENT_TIMEOUT),
        }
    }
}

impl Executor for Session {
    type Error = ExecutionError;
    fn query(&self, sql: &str, parameters: &[Parameter]) -> Result<Vec<RecordBatch>, Self::Error> {
        match self {
            Self::Embedded(Some(connection)) => connection
                .query(sql, parameters)
                .map_err(|_| Failure::Query.into()),
            Self::Embedded(None) => Err(Failure::Closed.into()),
            Self::Remote(session) => session.query(sql, parameters),
        }
    }
}

/// Cloneable cancellation control usable while another thread is executing.
#[derive(Clone)]
pub enum Cancellation {
    Embedded(Arc<duckdb::InterruptHandle>),
    Remote(RemoteCancellation),
}

impl Cancellation {
    pub fn cancel(&self) -> Result<(), ExecutionError> {
        match self {
            Self::Embedded(handle) => {
                handle.interrupt();
                Ok(())
            }
            Self::Remote(control) => {
                if control.closed.load(Ordering::Acquire) {
                    return Err(Failure::Closed.into());
                }
                control
                    .transport
                    .request(
                        Method::POST,
                        &format!("{}/cancel", control.path),
                        None,
                        STATEMENT_TIMEOUT,
                        false,
                    )
                    .map(|_| ())
            }
        }
    }
}

#[derive(Clone)]
pub struct RemoteCancellation {
    transport: Transport,
    path: String,
    closed: Arc<AtomicBool>,
}

pub struct RemoteSession {
    transport: Transport,
    path: String,
    closed: Arc<AtomicBool>,
    lost: AtomicBool,
    cleanup_complete: bool,
    task: Option<TaskOptions>,
    journal: Option<Arc<dyn Journal>>,
    commit_sent: AtomicBool,
    commit_returned: AtomicBool,
    request_sent: AtomicBool,
}

/// Only an exact COMMIT statement publishes; generated SQL never commits
/// implicitly inside the explicit transaction the golden task opens.
fn is_commit(sql: &str) -> bool {
    let sql = sql.trim().trim_end_matches(';').trim();
    sql.eq_ignore_ascii_case("COMMIT") || sql.eq_ignore_ascii_case("COMMIT TRANSACTION")
}

impl RemoteSession {
    fn open(transport: Transport) -> Result<Self, ExecutionError> {
        Self::open_with(transport, None, None)
    }

    fn open_task(
        transport: Transport,
        options: TaskOptions,
        journal: Option<Arc<dyn Journal>>,
    ) -> Result<Self, ExecutionError> {
        options.validate()?;
        Self::open_with(transport, Some(options), journal)
    }

    fn commit_state(&self) -> CommitState {
        if self.commit_returned.load(Ordering::Acquire) {
            CommitState::Returned
        } else if self.commit_sent.load(Ordering::Acquire) {
            CommitState::Sent
        } else {
            CommitState::NotSent
        }
    }

    fn open_with(
        transport: Transport,
        task: Option<TaskOptions>,
        journal: Option<Arc<dyn Journal>>,
    ) -> Result<Self, ExecutionError> {
        let payload = task
            .as_ref()
            .map_or_else(|| json!({}), TaskOptions::payload);
        let bytes = transport.request(
            Method::POST,
            "/v1/sessions",
            Some(&payload),
            Duration::from_secs(450),
            false,
        )?;
        let response: Value = serde_json::from_slice(&bytes).map_err(|_| Failure::Protocol)?;
        let id = response
            .get("session_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or(Failure::Protocol)?;
        let mut session = Self {
            transport,
            path: format!("/v1/sessions/{id}"),
            closed: Arc::new(AtomicBool::new(false)),
            lost: AtomicBool::new(false),
            cleanup_complete: false,
            task,
            journal,
            commit_sent: AtomicBool::new(false),
            commit_returned: AtomicBool::new(false),
            request_sent: AtomicBool::new(false),
        };
        if session
            .task
            .as_ref()
            .is_some_and(|options| !options.matches(&response["placement"]))
        {
            let _ = session.close(Duration::from_secs(3));
            return Err(Failure::Capacity.into());
        }
        if let Some(journal) = &session.journal
            && journal.session(id).is_err()
        {
            // No SQL has been sent; the session holds no transaction.
            let _ = session.close(Duration::from_secs(3));
            return Err(Failure::Journal.into());
        }
        Ok(session)
    }

    fn close(&mut self, timeout: Duration) -> Result<(), ExecutionError> {
        self.closed.store(true, Ordering::Release);
        if self.cleanup_complete {
            return Ok(());
        }
        let bytes = self
            .transport
            .request(Method::DELETE, &self.path, None, timeout, false)?;
        let response: Value = serde_json::from_slice(&bytes).map_err(|_| Failure::Protocol)?;
        // A missing gateway route does not prove that its worker stopped. Only
        // the upstream's explicit closure acknowledgement completes cleanup.
        if response.get("closed").and_then(Value::as_bool) != Some(true) {
            return Err(Failure::UnknownOutcome.into());
        }
        self.cleanup_complete = true;
        Ok(())
    }
}

impl Executor for RemoteSession {
    type Error = ExecutionError;
    fn query(&self, sql: &str, parameters: &[Parameter]) -> Result<Vec<RecordBatch>, Self::Error> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Failure::Closed.into());
        }
        if self.lost.load(Ordering::Acquire) {
            return Err(Failure::SessionLost.into());
        }
        if sql.len() > 262_144 || parameters.len() > 10_000 {
            return Err(Failure::Limit.into());
        }
        let parameters = parameters
            .iter()
            .map(|parameter| match parameter {
                Parameter::Null => Ok(Value::Null),
                Parameter::Boolean(value) => Ok(json!(value)),
                Parameter::Bigint(value) => Ok(json!(value)),
                Parameter::Double(value) if value.is_finite() => Ok(json!(value)),
                Parameter::Text(value) => Ok(json!(value)),
                Parameter::Double(_) | Parameter::ListVarchar(_) => Err(Failure::Parameter),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let request_id = Uuid::now_v7();
        let commit = is_commit(sql);
        if let Some(journal) = &self.journal {
            if journal.request(request_id, commit).is_err() {
                // Never sent. The owner is fenced or cannot record evidence, so
                // no later request on this session may be sent either.
                self.lost.store(true, Ordering::Release);
                return Err(ExecutionError {
                    kind: Failure::Journal,
                    request_id: Some(request_id),
                });
            }
        } else if commit && self.task.is_some() {
            // An unjournaled task commit would be unreconcilable after loss.
            tracing::debug!("task commit sent without a recovery journal");
        }
        self.request_sent.store(true, Ordering::Release);
        if commit {
            self.commit_sent.store(true, Ordering::Release);
        }
        let span = tracing::info_span!("duckfleet.query",
            er.query_id = %request_id,
            er.job_id = self.task.as_ref().map(|task| task.job_id.as_str()).unwrap_or_default(),
            er.task_id = self.task.as_ref().map(|task| task.task_id.as_str()).unwrap_or_default(),
            er.task_instance_id = self.task.as_ref().map(|task| task.attempt_id.as_str()).unwrap_or_default(),
            error.type = tracing::field::Empty,
        );
        let _entered = span.enter();
        let seconds = self
            .task
            .as_ref()
            .map_or(60, |options| options.statement_seconds);
        let context = self
            .task
            .as_ref()
            .map_or_else(|| json!({}), TaskOptions::context);
        let payload = json!({"request_id": request_id.to_string(), "sql": sql, "parameters": parameters, "timeout_seconds": seconds, "context": context});
        let result = self.transport.request(
            Method::POST,
            &format!("{}/queries", self.path),
            Some(&payload),
            self.task
                .as_ref()
                .map_or(STATEMENT_TIMEOUT, |_| Duration::from_secs(seconds + 135)),
            true,
        );
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(mut error) => {
                if error.kind == Failure::Transport {
                    error.kind = Failure::UnknownOutcome;
                }
                if matches!(
                    error.kind,
                    Failure::UnknownOutcome
                        | Failure::Cancelled
                        | Failure::Limit
                        | Failure::SessionLost
                        | Failure::Unauthorized
                        | Failure::Protocol
                ) {
                    self.lost.store(true, Ordering::Release);
                }
                span.record("error.type", error.kind.to_string());
                error.request_id = Some(request_id);
                return Err(error);
            }
        };
        if commit {
            // The service returned a statement result for COMMIT. Decoding the
            // (empty) result cannot un-publish it.
            self.commit_returned.store(true, Ordering::Release);
        }
        let decoded = StreamReader::try_new(bytes.as_slice(), None)
            .and_then(Iterator::collect::<Result<Vec<_>, _>>);
        match decoded {
            Ok(batches) if batches.iter().map(RecordBatch::num_rows).sum::<usize>() <= 100_000 => {
                Ok(batches)
            }
            _ => {
                self.lost.store(true, Ordering::Release);
                Err(ExecutionError {
                    kind: Failure::Protocol,
                    request_id: Some(request_id),
                })
            }
        }
    }
}

impl Drop for RemoteSession {
    fn drop(&mut self) {
        if !self.cleanup_complete {
            let _ = self.close(Duration::from_secs(3));
        }
    }
}

#[cfg(test)]
mod tests;
