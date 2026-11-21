//! The remote-execution fence and its write-ahead journal (#1133).
//!
//! A remote task acquires a `remote_attempts` row before it opens a DuckFleet
//! session, journals each request id before sending it, and releases the row
//! only with an established publication outcome. While a row is unresolved,
//! `job/update/claim.sql` dispatches no writer of any backend for its dataset.
//! Rows the pod could not settle are reconciled here, by the dispatcher sweep,
//! from durable upstream evidence — never by replaying SQL.
//!
//! The route is on the attempt channel: authenticated by the attempt token and
//! scoped by the task row it names, never by anything in the body. Bodies carry
//! identifiers and bounded phases only.

use std::time::Duration;

use er_duckfleet::Uuid;
use serde::Deserialize;

/// The journal wire body. Flat, because the OpenAPI derivation describes
/// plain structs: `op` selects which of the optional fields are required, and
/// `parse` refuses any other combination before anything is stored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AttemptRemote {
    /// `acquire`, `session`, `request`, `unknown` or `release`.
    op: String,
    /// The remote attempt UUID the pod minted before acquiring.
    id: String,
    /// The pod's own identity, honoured only on the fallback-token path.
    #[serde(default)]
    pod_uid: Option<String>,
    #[serde(default)]
    binding_generation: Option<u64>,
    #[serde(default)]
    namespace: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    commit: Option<bool>,
    /// `committed`, `rolled_back` or `not_executed`; never `unknown`.
    #[serde(default)]
    phase: Option<String>,
}

/// One validated journal operation.
pub(crate) enum Journal {
    Acquire {
        id: String,
        pod_uid: String,
        binding_generation: i64,
        namespace: String,
    },
    Session {
        id: String,
        pod_uid: String,
        session_id: String,
    },
    Request {
        id: String,
        pod_uid: String,
        request_id: String,
        commit: bool,
    },
    Unknown {
        id: String,
    },
    Release {
        id: String,
        phase: &'static str,
    },
}

fn uuid(value: String) -> Result<String, &'static str> {
    Uuid::parse_str(&value)
        .map(|_| value)
        .map_err(|_| "identifiers must be UUIDs")
}

fn namespace_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl AttemptRemote {
    /// Identifiers only: anything that is not a UUID (or a bounded namespace
    /// name) is refused, so no free text — no SQL, no credential — is stored.
    ///
    /// # Errors
    /// A missing, extra or malformed field for the operation.
    pub(crate) fn parse(self) -> Result<Journal, &'static str> {
        let id = uuid(self.id)?;
        let extra = |present: bool| {
            if present {
                Err("unexpected field")
            } else {
                Ok(())
            }
        };
        match self.op.as_str() {
            "acquire" => {
                extra(self.session_id.is_some() || self.request_id.is_some())?;
                extra(self.commit.is_some() || self.phase.is_some())?;
                let namespace = self.namespace.ok_or("namespace is required")?;
                if !namespace_ok(&namespace) {
                    return Err("invalid namespace");
                }
                Ok(Journal::Acquire {
                    id,
                    pod_uid: self.pod_uid.ok_or("pod_uid is required")?,
                    binding_generation: self
                        .binding_generation
                        .and_then(|g| i64::try_from(g).ok())
                        .filter(|g| *g > 0)
                        .ok_or("invalid binding generation")?,
                    namespace,
                })
            }
            "session" => {
                extra(self.request_id.is_some() || self.commit.is_some())?;
                extra(self.phase.is_some() || self.namespace.is_some())?;
                extra(self.binding_generation.is_some())?;
                Ok(Journal::Session {
                    id,
                    pod_uid: self.pod_uid.ok_or("pod_uid is required")?,
                    session_id: uuid(self.session_id.ok_or("session_id is required")?)?,
                })
            }
            "request" => {
                extra(self.session_id.is_some() || self.phase.is_some())?;
                extra(self.namespace.is_some() || self.binding_generation.is_some())?;
                Ok(Journal::Request {
                    id,
                    pod_uid: self.pod_uid.ok_or("pod_uid is required")?,
                    request_id: uuid(self.request_id.ok_or("request_id is required")?)?,
                    commit: self.commit.ok_or("commit is required")?,
                })
            }
            "unknown" => {
                extra(self.pod_uid.is_some() || self.session_id.is_some())?;
                extra(self.request_id.is_some() || self.commit.is_some())?;
                extra(self.phase.is_some() || self.namespace.is_some())?;
                extra(self.binding_generation.is_some())?;
                Ok(Journal::Unknown { id })
            }
            "release" => {
                extra(self.pod_uid.is_some() || self.session_id.is_some())?;
                extra(self.request_id.is_some() || self.commit.is_some())?;
                extra(self.namespace.is_some() || self.binding_generation.is_some())?;
                let phase = match self.phase.as_deref() {
                    Some("committed") => "committed",
                    Some("rolled_back") => "rolled_back",
                    Some("not_executed") => "not_executed",
                    _ => return Err("phase must be committed, rolled_back or not_executed"),
                };
                Ok(Journal::Release { id, phase })
            }
            _ => Err("unknown journal operation"),
        }
    }
}

impl Journal {
    /// The pod identity the caller claims, for the fallback-token path.
    pub(crate) fn claimed_pod(&self) -> &str {
        match self {
            Self::Acquire { pod_uid, .. }
            | Self::Session { pod_uid, .. }
            | Self::Request { pod_uid, .. } => pod_uid,
            Self::Unknown { .. } | Self::Release { .. } => "",
        }
    }
}

/// The customer-safe refusal naming the held dataset fence.
pub(crate) fn held_detail(attempt: &str) -> String {
    format!(
        "this dataset is held by remote attempt {attempt}, whose publication outcome is not yet \
         established; it is released automatically once DuckFleet evidence settles it"
    )
}

/// Apply one journal operation for the authenticated task. `pod_uid` is the
/// verified identity (or the claimed one on the fallback path). `Ok(None)` is
/// recorded; `Ok(Some(detail))` is a refusal: the owner is no longer the live
/// registered pod, or (for acquire) another attempt holds the dataset.
///
/// # Errors
/// Propagates a Postgres failure.
pub(crate) async fn record(
    client: &tokio_postgres::Client,
    task: &str,
    pod_uid: &str,
    journal: &Journal,
) -> Result<Option<String>, tokio_postgres::Error> {
    use er_entities::remote_attempt::{create, read, update};
    let (op, written) = match journal {
        Journal::Acquire {
            id,
            binding_generation,
            namespace,
            ..
        } => (
            "acquire",
            create::acquire(client, id, task, pod_uid, *binding_generation, namespace).await?,
        ),
        Journal::Session { id, session_id, .. } => (
            "session",
            update::record_session(client, session_id, id, task, pod_uid).await?,
        ),
        Journal::Request {
            id,
            request_id,
            commit,
            ..
        } => (
            "request",
            update::record_request(client, request_id, *commit, id, task, pod_uid).await?,
        ),
        Journal::Unknown { id } => ("unknown", update::note_unknown(client, id, task).await?),
        Journal::Release { id, phase } => {
            ("release", update::release(client, phase, id, task).await?)
        }
    };
    if written == 1 {
        tracing::info!(er.task_instance_id = %task, op = op, "remote attempt journal recorded");
        return Ok(None);
    }
    let detail = match journal {
        Journal::Acquire { .. } => read::holding(client, task).await?.first().map_or_else(
            || {
                "fenced: this launch already made its remote attempt, or the task is no \
                 longer running as this pod"
                    .to_owned()
            },
            |held| held_detail(&held.id),
        ),
        _ => "fenced: the remote attempt is not owned by this live pod".to_owned(),
    };
    tracing::warn!(er.task_instance_id = %task, op = op, "remote attempt journal refused");
    Ok(Some(detail))
}

/// How long after an attempt's last journaled evidence an absent receipt and a
/// finished-or-unadmitted COMMIT may be read as a rollback. It must exceed the
/// longest transport wait a COMMIT delivery can still be in (statement deadline
/// up to 600 s plus 135 s of queue and delivery allowance) and the time a dying
/// worker's catalog transaction takes to resolve.
pub(crate) const DEFAULT_GRACE: Duration = Duration::from_secs(900);

/// Operator-owned recovery configuration: the binding registry and projected
/// credentials the sweep reads evidence through, and the delivery grace. Unset
/// means attempts with a journaled COMMIT stay held — never guessed at.
#[derive(Clone)]
pub struct Recovery {
    registry: std::path::PathBuf,
    credentials: std::path::PathBuf,
    grace: Duration,
}

impl Recovery {
    /// `ER_DUCKFLEET_RECOVERY_REGISTRY` and `ER_DUCKFLEET_RECOVERY_CREDENTIALS_DIR`
    /// (absolute), and optionally `ER_DUCKFLEET_RECOVERY_GRACE_SECONDS`.
    ///
    /// # Errors
    /// Relative paths, a half-configured pair or an unparsable grace.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Option<Result<Self, String>> {
        let registry = lookup("ER_DUCKFLEET_RECOVERY_REGISTRY");
        let credentials = lookup("ER_DUCKFLEET_RECOVERY_CREDENTIALS_DIR");
        if registry.is_none() && credentials.is_none() {
            return None;
        }
        let grace = match lookup("ER_DUCKFLEET_RECOVERY_GRACE_SECONDS") {
            None => DEFAULT_GRACE,
            Some(value) => match value.parse::<u64>() {
                Ok(seconds) if seconds > 0 => Duration::from_secs(seconds),
                _ => return Some(Err("invalid DuckFleet recovery grace".to_owned())),
            },
        };
        Some(match (registry, credentials) {
            (Some(registry), Some(credentials))
                if std::path::Path::new(&registry).is_absolute()
                    && std::path::Path::new(&credentials).is_absolute() =>
            {
                Ok(Self {
                    registry: registry.into(),
                    credentials: credentials.into(),
                    grace,
                })
            }
            _ => {
                Err("DuckFleet recovery requires absolute registry and credential paths".to_owned())
            }
        })
    }
}

/// The service-side evidence for one attempt with a journaled COMMIT, read
/// through its pinned binding generation. Blocking; run off the runtime.
fn gather(
    recovery: &Recovery,
    attempt: &er_entities::remote_attempt::RemoteAttempt,
    commit_request: Uuid,
) -> (
    Result<bool, recovery::ReadFailed>,
    Result<recovery::RequestState, recovery::ReadFailed>,
) {
    use er_duckfleet::execution::Session;
    use er_duckfleet::execution::recovery::{ReadFailed, receipt_present, request_state};
    let Ok(registry) = er_duckfleet::Registry::load(&recovery.registry, &recovery.credentials)
    else {
        return (Err(ReadFailed), Err(ReadFailed));
    };
    let Ok(generation) = u64::try_from(attempt.binding_generation) else {
        return (Err(ReadFailed), Err(ReadFailed));
    };
    let Ok(binding) =
        registry.resolve_for_recovery(&attempt.org_id, &attempt.dataset_id, generation)
    else {
        return (Err(ReadFailed), Err(ReadFailed));
    };
    // Disposable integration harness only; never compiled into the product.
    #[cfg(feature = "disposable-proof")]
    if let Ok(address) = std::env::var("ER_DUCKFLEET_PROOF_ADDRESS")
        && let Ok(address) = address.parse::<std::net::SocketAddr>()
    {
        let commit = recovery::request_state_loopback_proof(
            address,
            binding.bearer_token().to_owned(),
            commit_request,
        );
        let receipt = Session::open_loopback_proof(address, binding.bearer_token().to_owned())
            .map_err(|_| ReadFailed)
            .and_then(|mut session| {
                let present = receipt_present(&session, &attempt.id);
                let _ = session.close();
                present
            });
        return (receipt, commit);
    }
    // The commit record first: a receipt read after a finished commit cannot
    // miss a publication that the record already reports as finished.
    let commit = request_state(&binding, commit_request);
    let receipt = Session::open_remote(&binding)
        .map_err(|_| ReadFailed)
        .and_then(|mut session| {
            let present = receipt_present(&session, &attempt.id);
            let _ = session.close();
            present
        });
    (receipt, commit)
}

use er_duckfleet::execution::recovery;

/// Reconcile every unresolved attempt whose owning launch can no longer
/// submit. Releases only on a conclusive decision; holds are logged with
/// their bounded reason and retried next sweep. Returns the number released.
///
/// # Errors
/// Propagates a Postgres failure; evidence failures are holds, not errors.
pub async fn sweep(
    client: &tokio_postgres::Client,
    recovery: Option<&Result<Recovery, String>>,
) -> Result<usize, tokio_postgres::Error> {
    match recovery {
        Some(Ok(config)) => {
            let config = config.clone();
            let grace = config.grace;
            let gather: Gatherer =
                std::sync::Arc::new(move |attempt, request| gather(&config, attempt, request));
            sweep_with(client, grace, Some(gather)).await
        }
        _ => sweep_with(client, DEFAULT_GRACE, None).await,
    }
}

/// Reads the service-side evidence for one attempt and its journaled COMMIT.
pub type Gatherer = std::sync::Arc<
    dyn Fn(
            &er_entities::remote_attempt::RemoteAttempt,
            Uuid,
        ) -> (
            Result<bool, recovery::ReadFailed>,
            Result<recovery::RequestState, recovery::ReadFailed>,
        ) + Send
        + Sync,
>;

/// `sweep` with an explicit evidence source and grace. `None` means no
/// recovery binding is configured: attempts with a journaled COMMIT hold.
///
/// # Errors
/// Propagates a Postgres failure; evidence failures are holds, not errors.
pub async fn sweep_with(
    client: &tokio_postgres::Client,
    grace: Duration,
    gather: Option<Gatherer>,
) -> Result<usize, tokio_postgres::Error> {
    use er_entities::remote_attempt::{read, update};
    let configured = gather.is_some();
    let mut released = 0;
    for attempt in read::settleable(client).await? {
        let quiet_for = (chrono::Utc::now() - attempt.updated_at)
            .to_std()
            .unwrap_or_default();
        let commit_request = attempt
            .commit_request_id
            .as_deref()
            .and_then(|id| Uuid::parse_str(id).ok());
        // Without a journaled COMMIT the decision needs no service evidence;
        // without recovery configured there is none to read, and it holds.
        let (receipt, commit) = match (commit_request, &gather) {
            (Some(request), Some(gather)) => {
                let gather = std::sync::Arc::clone(gather);
                let row = attempt.clone();
                let (receipt, commit) =
                    er_obs::spawn_blocking_in_span(move || gather(&row, request))
                        .await
                        .unwrap_or((Err(recovery::ReadFailed), Err(recovery::ReadFailed)));
                (Some(receipt), Some(commit))
            }
            _ => (None, None),
        };
        let decision = recovery::decide(&recovery::Evidence {
            owner_live: false,
            request_journaled: attempt.request_id.is_some(),
            commit_journaled: attempt.commit_request_id.is_some(),
            receipt,
            commit,
            quiet_for,
            grace,
        });
        if let Some(phase) = decision.phase() {
            let rows =
                update::release(client, phase, &attempt.id, &attempt.task_instance_id).await?;
            released += usize::try_from(rows).unwrap_or(0);
            tracing::info!(
                remote_attempt = %attempt.id,
                er.job_id = %attempt.job_id,
                er.task_instance_id = %attempt.task_instance_id,
                er.dataset = %attempt.dataset_id,
                outcome = phase,
                "remote attempt reconciled and its dataset released"
            );
        } else {
            let reason = match decision {
                recovery::Decision::Hold(hold) => hold.to_string(),
                _ => String::new(),
            };
            tracing::warn!(
                remote_attempt = %attempt.id,
                er.job_id = %attempt.job_id,
                er.task_instance_id = %attempt.task_instance_id,
                er.dataset = %attempt.dataset_id,
                hold = %reason,
                recovery_configured = configured,
                "remote attempt still holds its dataset"
            );
        }
    }
    Ok(released)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(value: serde_json::Value) -> Result<Journal, &'static str> {
        serde_json::from_value::<AttemptRemote>(value)
            .map_err(|_| "rejected by serde")?
            .parse()
    }

    #[test]
    fn the_journal_stores_identifiers_only() {
        let id = Uuid::now_v7().to_string();
        assert!(matches!(
            wire(
                serde_json::json!({"op":"acquire","id":id,"pod_uid":"p","binding_generation":3,"namespace":"ns-a_1"})
            ),
            Ok(Journal::Acquire {
                binding_generation: 3,
                ..
            })
        ));
        for bad in [
            serde_json::json!({"op":"acquire","id":"select 1","pod_uid":"p","binding_generation":3,"namespace":"ns"}),
            serde_json::json!({"op":"acquire","id":id,"pod_uid":"p","binding_generation":0,"namespace":"ns"}),
            serde_json::json!({"op":"acquire","id":id,"pod_uid":"p","binding_generation":3,"namespace":"ns; drop"}),
            serde_json::json!({"op":"acquire","id":id,"pod_uid":"p","binding_generation":3,"namespace":"ns","phase":"committed"}),
            serde_json::json!({"op":"request","id":id,"pod_uid":"p","request_id":"COMMIT","commit":true}),
            serde_json::json!({"op":"request","id":id,"pod_uid":"p","request_id":id}),
            serde_json::json!({"op":"session","id":id,"pod_uid":"p"}),
            serde_json::json!({"op":"release","id":id,"phase":"unknown"}),
            serde_json::json!({"op":"release","id":id}),
            serde_json::json!({"op":"release","id":id,"phase":"committed","sql":"x"}),
            serde_json::json!({"op":"replay","id":id}),
        ] {
            assert!(wire(bad.clone()).is_err(), "{bad}");
        }
        assert!(matches!(
            wire(serde_json::json!({"op":"release","id":id,"phase":"not_executed"})),
            Ok(Journal::Release {
                phase: "not_executed",
                ..
            })
        ));
    }

    #[test]
    fn the_held_detail_names_the_attempt_and_nothing_from_the_service() {
        let detail = held_detail("0190-a");
        assert!(detail.contains("0190-a"));
        assert!(!detail.to_lowercase().contains("select"));
    }

    #[test]
    fn recovery_is_operator_configured_and_refuses_partial_configuration() {
        let lookup = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert!(Recovery::from_lookup(lookup(&[])).is_none());
        let full = Recovery::from_lookup(lookup(&[
            ("ER_DUCKFLEET_RECOVERY_REGISTRY", "/etc/r.json"),
            ("ER_DUCKFLEET_RECOVERY_CREDENTIALS_DIR", "/etc/c"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(full.grace, DEFAULT_GRACE);
        for bad in [
            &[("ER_DUCKFLEET_RECOVERY_REGISTRY", "/etc/r.json")][..],
            &[
                ("ER_DUCKFLEET_RECOVERY_REGISTRY", "r.json"),
                ("ER_DUCKFLEET_RECOVERY_CREDENTIALS_DIR", "/etc/c"),
            ][..],
            &[
                ("ER_DUCKFLEET_RECOVERY_REGISTRY", "/etc/r.json"),
                ("ER_DUCKFLEET_RECOVERY_CREDENTIALS_DIR", "/etc/c"),
                ("ER_DUCKFLEET_RECOVERY_GRACE_SECONDS", "0"),
            ][..],
        ] {
            assert!(
                matches!(Recovery::from_lookup(lookup(bad)), Some(Err(_))),
                "{bad:?}"
            );
        }
    }
}
