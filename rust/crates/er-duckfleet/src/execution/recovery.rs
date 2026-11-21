//! Restart-safe reconciliation of a remote attempt whose publication outcome
//! its owner could not establish. Evidence comes from three places, none of
//! which is a replay: the journal (which request ids may have been sent), the
//! service's durable request records, and the publication receipt that the
//! golden transaction writes into the lake commit itself.
//!
//! `decide` is the failure matrix. It is pure so every row can be tested; the
//! gathering functions below only collect its inputs.

use std::io::Read;
use std::time::Duration;

use arrow::array::{Array, Int64Array};
use er_entities::execution::{Executor, Parameter};
use reqwest::Method;
use serde_json::{Value, json};
use uuid::Uuid;

use super::Transport;
use crate::Binding;

/// The lake commit tag naming the attempt that published. DuckLake stores it in
/// the snapshot row written by COMMIT, so it exists exactly when the
/// transaction committed (probed on DuckDB 1.5.4 and 2.0.0).
#[must_use]
pub fn receipt(attempt_id: &str) -> String {
    json!({ "remote_attempt": attempt_id }).to_string()
}

/// The statement the publishing transaction executes before COMMIT.
pub const RECEIPT_SQL: &str = "CALL lake.set_commit_message('er', 'golden', extra_info => ?)";

/// Whether the attempt's publication receipt is in the lake's snapshot history.
///
/// # Errors
/// Propagates execution failures and refuses an unexpected result shape.
pub fn receipt_present<E: Executor>(session: &E, attempt_id: &str) -> Result<bool, ReadFailed> {
    let batches = session
        .query(
            "SELECT count(*)::BIGINT FROM ducklake_snapshots('lake') WHERE commit_extra_info = ?",
            &[Parameter::Text(receipt(attempt_id))],
        )
        .map_err(|_| ReadFailed)?;
    batches
        .first()
        .filter(|batch| batch.num_columns() == 1 && batch.num_rows() == 1)
        .and_then(|batch| batch.column(0).as_any().downcast_ref::<Int64Array>())
        .filter(|array| !array.is_null(0))
        .map(|array| array.value(0) > 0)
        .ok_or(ReadFailed)
}

/// Evidence could not be read. Deliberately carries nothing from the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("recovery evidence unavailable")]
pub struct ReadFailed;

/// The service's durable account of one request id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestState {
    /// No record visible to this binding. Conclusive only once no in-flight
    /// delivery of that request can still be admitted (the grace period).
    NotFound,
    /// Accepted or executing: the statement may still take effect.
    Pending,
    /// The service has finished with it.
    Terminal(RequestOutcome),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestOutcome {
    NotExecuted,
    StatementResult,
    Unknown,
}

fn parse_request(value: &Value) -> Option<RequestState> {
    match value.get("state")?.as_str()? {
        "accepted" | "executing" => Some(RequestState::Pending),
        "terminal" => Some(RequestState::Terminal(
            match value.get("outcome")?.as_str()? {
                "not_executed" => RequestOutcome::NotExecuted,
                "statement_result" => RequestOutcome::StatementResult,
                "unknown" => RequestOutcome::Unknown,
                _ => return None,
            },
        )),
        _ => None,
    }
}

/// Read `GET /v1/requests/{id}` with the attempt's pinned binding. The lookup
/// is owner-scoped upstream, so a different credential would see `NotFound`
/// for a request that exists — which is why the binding must be the pinned one.
///
/// # Errors
/// Transport, authorization and protocol failures are not evidence.
pub fn request_state(binding: &Binding<'_>, request_id: Uuid) -> Result<RequestState, ReadFailed> {
    let transport = Transport::new(binding.endpoint(), binding.bearer_token().to_owned())
        .map_err(|_| ReadFailed)?;
    request_state_with(&transport, request_id)
}

/// Loopback proof variant of `request_state`.
#[cfg(feature = "disposable-proof")]
pub fn request_state_loopback_proof(
    address: std::net::SocketAddr,
    token: String,
    request_id: Uuid,
) -> Result<RequestState, ReadFailed> {
    if !address.ip().is_loopback() {
        return Err(ReadFailed);
    }
    let transport = Transport::new(format!("http://{address}"), token).map_err(|_| ReadFailed)?;
    request_state_with(&transport, request_id)
}

pub(super) fn request_state_with(
    transport: &Transport,
    request_id: Uuid,
) -> Result<RequestState, ReadFailed> {
    let response = transport
        .http
        .request(
            Method::GET,
            format!("{}/v1/requests/{request_id}", transport.endpoint),
        )
        .bearer_auth(&transport.token)
        .timeout(Duration::from_secs(30))
        .send()
        .map_err(|_| ReadFailed)?;
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    response
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ReadFailed)?;
    if bytes.len() > 64 * 1024 {
        return Err(ReadFailed);
    }
    match status {
        200 => serde_json::from_slice::<Value>(&bytes)
            .ok()
            .as_ref()
            .and_then(parse_request)
            .ok_or(ReadFailed),
        404 => Ok(RequestState::NotFound),
        _ => Err(ReadFailed),
    }
}

/// Everything `decide` may rely on, gathered without replaying anything.
#[derive(Debug, Clone, Copy)]
pub struct Evidence {
    /// The owning launch is still live and running, so it may still journal
    /// and send. Its own settlement is authoritative while it lives.
    pub owner_live: bool,
    /// Whether any request id was ever journaled.
    pub request_journaled: bool,
    /// Whether a COMMIT request id was journaled (and so may have been sent).
    pub commit_journaled: bool,
    /// The receipt read, when a COMMIT was journaled. `None`: not read.
    pub receipt: Option<Result<bool, ReadFailed>>,
    /// The service's record of the journaled COMMIT. `None`: not read.
    pub commit: Option<Result<RequestState, ReadFailed>>,
    /// Time since the attempt's last journaled evidence.
    pub quiet_for: Duration,
    /// How long an in-flight delivery or a dying worker's catalog commit can
    /// still land after the evidence stopped changing.
    pub grace: Duration,
}

/// The established publication outcome, or why the fence must stay held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Committed,
    RolledBack,
    NotExecuted,
    Hold(Hold),
}

/// A bounded, customer-safe reason. Never carries service text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Hold {
    #[error("the owning task may still submit")]
    OwnerLive,
    #[error("the publication receipt could not be read")]
    ReceiptUnavailable,
    #[error("the commit request record could not be read")]
    CommitUnavailable,
    #[error("the commit request is still executing")]
    CommitPending,
    #[error("the commit reported success but no publication receipt exists")]
    Contradiction,
    #[error("waiting out the delivery grace period")]
    Grace,
}

impl Decision {
    /// The `remote_attempts.phase` written when the fence is released.
    #[must_use]
    pub fn phase(self) -> Option<&'static str> {
        match self {
            Self::Committed => Some("committed"),
            Self::RolledBack => Some("rolled_back"),
            Self::NotExecuted => Some("not_executed"),
            Self::Hold(_) => None,
        }
    }
}

/// The failure matrix. Release requires proof that no publication can still
/// happen; anything weaker holds the dataset fence.
///
/// * A receipt proves publication, whoever is still alive.
/// * While the owner lives, only the owner settles.
/// * A COMMIT never journaled was never sent (journal-before-send), and the
///   owner can no longer journal, so nothing can publish.
/// * A journaled COMMIT with no receipt is rolled back only once the service
///   has finished with it (or never admitted it) and the grace has elapsed.
#[must_use]
pub fn decide(evidence: &Evidence) -> Decision {
    if matches!(evidence.receipt, Some(Ok(true))) {
        return Decision::Committed;
    }
    if evidence.owner_live {
        return Decision::Hold(Hold::OwnerLive);
    }
    if !evidence.commit_journaled {
        return if evidence.request_journaled {
            Decision::RolledBack
        } else {
            Decision::NotExecuted
        };
    }
    match evidence.receipt {
        Some(Ok(false)) => {}
        _ => return Decision::Hold(Hold::ReceiptUnavailable),
    }
    match evidence.commit {
        Some(Ok(RequestState::Pending)) => Decision::Hold(Hold::CommitPending),
        Some(Ok(RequestState::Terminal(RequestOutcome::StatementResult))) => {
            Decision::Hold(Hold::Contradiction)
        }
        Some(Ok(
            RequestState::NotFound
            | RequestState::Terminal(RequestOutcome::NotExecuted | RequestOutcome::Unknown),
        )) => {
            if evidence.quiet_for >= evidence.grace {
                Decision::RolledBack
            } else {
                Decision::Hold(Hold::Grace)
            }
        }
        Some(Err(_)) | None => Decision::Hold(Hold::CommitUnavailable),
    }
}

/// Decide for an owner that has just finished, from what it observed itself.
/// Conclusive only when COMMIT returned or was never sent; otherwise `None`,
/// and the fence waits for `decide` with the service's records.
#[must_use]
pub fn decide_owner(commit: super::CommitState, request_journaled: bool) -> Option<Decision> {
    match commit {
        super::CommitState::Returned => Some(Decision::Committed),
        super::CommitState::NotSent if request_journaled => Some(Decision::RolledBack),
        super::CommitState::NotSent => Some(Decision::NotExecuted),
        super::CommitState::Sent => None,
    }
}

#[cfg(test)]
mod tests;
