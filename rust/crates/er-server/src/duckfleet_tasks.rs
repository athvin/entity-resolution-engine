//! Default-off, operator-owned selection for remote pod tasks.
//!
//! [`route`] is the whole routing policy: every pod task of every DAG maps to
//! [`Route::Remote`] or to [`Route::Embedded`] with the reason it stays local.
//! There is no default arm, and `every_pod_task_has_an_explicit_route` fails
//! when a DAG gains a task this table does not name. An enrolled pod therefore
//! never runs a task embedded without saying why.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use er_dag::{DurableOutput, LakeRef, Plan, TaskError, WorkOrder};
use er_duckfleet::Uuid;
use er_duckfleet::execution::recovery::{self, Decision};
use er_duckfleet::execution::{Compute, Journal, JournalRefused, Session, TaskOptions};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::dags::{JobKind, PodTaskRun, edges::Golden, plans::RunPlan};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    registry: PathBuf,
    credentials: PathBuf,
    statement_seconds: u64,
    classes: BTreeMap<String, Compute>,
}

fn configuration(message: &str) -> TaskError {
    TaskError::Failed(er_errors::ErError::config(message.to_owned()))
}

/// Where a pod task runs when its pod is enrolled in a DuckFleet task policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// Through the remote adapter, with the attempt fence, journal and receipt.
    Remote,
    /// In the pod's embedded engine, for the stated reason.
    Embedded(&'static str),
}

/// Lake administration: schema reconciliation, catalog creation and
/// maintenance belong to ER's embedded reconciler, which owns the catalog
/// version policy (no automatic migration).
const ADMINISTRATION: &str = "lake administration stays with the embedded reconciler";
/// Run bookkeeping that brackets the stages: no SQL workload to move.
const ORCHESTRATION: &str = "run bookkeeping, not a remote workload";
/// A stage with no qualified remote adapter yet. It runs embedded under the
/// same job slot, so there is still one writer per dataset.
const NOT_QUALIFIED: &str = "no qualified remote adapter for this task";

/// The routing table. Remote is granted only where `make check-duckfleet-task`
/// qualifies it: account golden inside `run_all`. `golden` in the correction
/// DAGs shares the body but has not been qualified remotely, so it stays
/// embedded. `None` means the table does not name the task.
pub(crate) fn route(kind: JobKind, task: &str) -> Option<Route> {
    use JobKind as K;
    let run_all = matches!(kind, K::RunAllFull | K::RunAllIncremental);
    let route = match task {
        "golden" if run_all => Route::Remote,
        "golden" if matches!(kind, K::Correct | K::ApplyCorrections) => {
            Route::Embedded(NOT_QUALIFIED)
        }
        "reconcile_schema" | "lake_init" | "maintain" => Route::Embedded(ADMINISTRATION),
        "init" | "assemble" | "finish" => Route::Embedded(ORCHESTRATION),
        "ingest"
        | "standardize"
        | "match"
        | "reconcile"
        | "train"
        | "profile"
        | "extract"
        | "export"
        | "write_back_scan"
        | "apply_staged"
        | "materialize_previews" => Route::Embedded(NOT_QUALIFIED),
        _ => return None,
    };
    Some(route)
}

/// A policy path comes from the task-pod deployment, never job parameters.
/// Without one, nothing is remote. With one, [`route`] decides, and an
/// embedded decision is logged with its reason rather than taken silently.
pub(crate) fn selected(
    kind: JobKind,
    task: &str,
    lookup: impl Fn(&str) -> Option<String>,
) -> Option<PathBuf> {
    let policy = lookup("ER_DUCKFLEET_TASK_POLICY").map(PathBuf::from)?;
    match route(kind, task) {
        Some(Route::Remote) => Some(policy),
        Some(Route::Embedded(reason)) => {
            tracing::info!(
                er.task_id = task,
                job.kind = kind.as_str(),
                reason,
                "DuckFleet-enrolled pod runs this task embedded"
            );
            None
        }
        None => None,
    }
}

/// The task pod as the owner of a remote attempt: its attempt channel and the
/// pod identity every journal write is fenced on.
pub(crate) struct Owner<'a> {
    pub(crate) channel: &'a crate::exec_task::Channel,
    pub(crate) attempt: &'a str,
    pub(crate) pod_uid: &'a str,
}

/// A refusal that is not transient: the owner is fenced or another attempt
/// holds the dataset. Carries the server's customer-safe detail.
struct Refused(String);

/// Post one journal operation, retrying only transient channel failures. A
/// write is durable once the server answers `recorded`; anything else is a
/// refusal, and the corresponding remote effect must not be started.
fn post_journal(
    channel: &crate::exec_task::Channel,
    attempt: &str,
    body: &Value,
) -> Result<(), Refused> {
    let path = format!("/v1/internal/attempts/{attempt}/remote");
    let mut last = "the attempt channel was unavailable".to_owned();
    for tick in 0..3 {
        match channel.post(&path, body) {
            Ok((200, reply)) if reply["recorded"] == true => return Ok(()),
            Ok((status, reply)) if !crate::exec_task::transient_channel_status(status) => {
                return Err(Refused(
                    reply["detail"]
                        .as_str()
                        .unwrap_or("the remote attempt journal was refused")
                        .to_owned(),
                ));
            }
            Ok((status, _)) => last = format!("the attempt channel returned {status}"),
            Err(error) => last = error,
        }
        if tick < 2 {
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }
    Err(Refused(last))
}

/// The DuckFleet write-ahead journal, backed by the attempt channel. Owns its
/// own channel client: the session holds it beyond the borrow of `Owner`.
struct ChannelJournal {
    channel: crate::exec_task::Channel,
    attempt: String,
    pod_uid: String,
    id: Uuid,
}

impl Journal for ChannelJournal {
    fn session(&self, session_id: Uuid) -> Result<(), JournalRefused> {
        post_journal(
            &self.channel,
            &self.attempt,
            &json!({"op":"session","id":self.id.to_string(),"pod_uid":self.pod_uid,"session_id":session_id.to_string()}),
        )
        .map_err(|_| JournalRefused)
    }
    fn request(&self, request_id: Uuid, commit: bool) -> Result<(), JournalRefused> {
        post_journal(
            &self.channel,
            &self.attempt,
            &json!({"op":"request","id":self.id.to_string(),"pod_uid":self.pod_uid,
                "request_id":request_id.to_string(),"commit":commit}),
        )
        .map_err(|_| JournalRefused)
    }
}

/// Settle the attempt from what this owner observed. Conclusive outcomes
/// release the fence; anything else is recorded as unknown and left to the
/// reconciliation sweep. Returns the error suffix a held dataset adds.
fn settle(owner: &Owner<'_>, id: Uuid, session: &Session) -> Option<String> {
    let decision = recovery::decide_owner(session.commit_state(), session.request_sent());
    let body = match decision.and_then(Decision::phase) {
        Some(phase) => json!({"op":"release","id":id.to_string(),"phase":phase}),
        None => json!({"op":"unknown","id":id.to_string()}),
    };
    let posted = post_journal(owner.channel, owner.attempt, &body);
    let held = decision.is_none() || posted.is_err();
    tracing::info!(
        er.task_instance_id = %owner.attempt,
        remote_attempt = %id,
        outcome = decision.and_then(Decision::phase).unwrap_or("unknown"),
        held = held,
        "remote attempt settled by its owner"
    );
    held.then(|| crate::remote_attempts::held_detail(&id.to_string()))
}

/// Execute without opening an embedded lake alongside the remote writer.
/// Errors follow the same task finish channel as embedded task errors.
pub(crate) fn run(
    order: &WorkOrder,
    owner: &Owner<'_>,
    policy: &std::path::Path,
    fence: &Arc<AtomicBool>,
    sigterm: &Arc<AtomicBool>,
) -> Result<(PodTaskRun, Option<i64>), TaskError> {
    if !policy.is_absolute() {
        return Err(configuration(
            "DuckFleet task policy must be an absolute operator-owned path",
        ));
    }
    let policy: Policy = std::fs::read(policy)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(|| configuration("invalid DuckFleet task policy"))?;
    if !policy.registry.is_absolute() || !policy.credentials.is_absolute() {
        return Err(configuration("DuckFleet binding paths must be absolute"));
    }
    let class = order.resource_class.as_deref().unwrap_or("M");
    let compute = policy
        .classes
        .get(class)
        .cloned()
        .ok_or_else(|| configuration("DuckFleet task resource class is not supported"))?;
    let dataset = order
        .dataset_id
        .as_deref()
        .ok_or_else(|| configuration("DuckFleet tasks require a dataset"))?;
    let models = crate::runctx::models_from_order(&order.models);
    let run = crate::runctx::RunContext::resolve(&crate::runctx::ResolveRequest {
        lake_env: &order.lake_env,
        params: &order.params,
        models: &models,
        drop_root: order.drop_root.as_deref(),
        dataset_id: Some(dataset),
        wants_deliveries: false,
    })
    .map_err(|_| configuration("could not resolve DuckFleet golden inputs"))?;
    if run.config.entity_type.is_person() {
        return Err(configuration(
            "DuckFleet golden pilot supports account datasets only",
        ));
    }
    let plan = order
        .input
        .get("pins")
        .and_then(|pins| pins.get(0))
        .ok_or_else(|| configuration("missing golden run plan"))?;
    let plan =
        Plan::<RunPlan>::rehydrate(plan).map_err(|_| configuration("invalid golden run plan"))?;
    // Rehydrate both declared pins even though the task reads the lake head
    // under the existing dataset writer slot.
    let resolved = order
        .input
        .get("pins")
        .and_then(|pins| pins.get(1))
        .ok_or_else(|| configuration("missing resolved lake reference"))?;
    let _ = LakeRef::<crate::dags::edges::Resolved>::rehydrate(resolved)
        .map_err(|_| configuration("invalid resolved lake reference"))?;
    let registry = er_duckfleet::Registry::load(&policy.registry, &policy.credentials)
        .map_err(|_| configuration("DuckFleet binding registry unavailable"))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| configuration("invalid system clock"))?
        .as_secs();
    let binding = registry
        .resolve_current(&order.org_id, dataset, now)
        .map_err(|_| configuration("DuckFleet dataset binding unavailable"))?;
    let options = TaskOptions {
        compute,
        statement_seconds: policy.statement_seconds,
        job_id: order.job_id.clone(),
        task_id: order.task_id.clone(),
        attempt_id: owner.attempt.to_owned(),
    };
    let threads = std::env::var("ER_DUCKDB_THREADS")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| configuration("DuckFleet task requires an explicit thread budget"))?;
    let memory = std::env::var("ER_DUCKDB_MEMORY_LIMIT")
        .ok()
        .and_then(|value| memory_bytes(&value))
        .ok_or_else(|| {
            configuration("DuckFleet task requires an explicit supported memory budget")
        })?;
    options.require_capacity(threads, memory).map_err(|_| {
        configuration("DuckFleet placement is smaller than the task resource budget")
    })?;
    let stage_error =
        |message: String| TaskError::Failed(er_errors::ErError::stage_failure(message));
    // The fence comes first: no session opens, and no SQL can be sent, until
    // this pod durably owns the dataset's remote attempt.
    let id = Uuid::now_v7();
    post_journal(
        owner.channel,
        owner.attempt,
        &json!({"op":"acquire","id":id.to_string(),"pod_uid":owner.pod_uid,
            "binding_generation":binding.generation(),"namespace":binding.namespace()}),
    )
    .map_err(|Refused(detail)| {
        TaskError::Failed(er_errors::ErError::precondition(format!(
            "remote execution was not admitted: {detail}"
        )))
    })?;
    let journal = std::sync::Arc::new(ChannelJournal {
        channel: crate::exec_task::Channel::new(
            owner.channel.base.clone(),
            owner.channel.token.clone(),
        )
        .map_err(|_| stage_error("could not build the remote attempt journal".into()))?,
        attempt: owner.attempt.to_owned(),
        pod_uid: owner.pod_uid.to_owned(),
        id,
    });
    let mut session = match open(&binding, options, journal) {
        Ok(session) => session,
        Err(error) => {
            // Nothing was sent; the fence is released as not executed. If even
            // that cannot be recorded, the sweep reaches the same conclusion.
            let _ = post_journal(
                owner.channel,
                owner.attempt,
                &json!({"op":"release","id":id.to_string(),"phase":"not_executed"}),
            );
            return Err(stage_error(error.to_string()));
        }
    };
    let control = session
        .cancellation()
        .map_err(|e| stage_error(e.to_string()))?;
    let finished = Arc::new(AtomicBool::new(false));
    let monitor = {
        let fence = Arc::clone(fence);
        let sigterm = Arc::clone(sigterm);
        let finished = Arc::clone(&finished);
        std::thread::spawn(move || {
            while !finished.load(Ordering::Acquire) {
                if fence.load(Ordering::Acquire) || sigterm.load(Ordering::Acquire) {
                    let _ = control.cancel();
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        })
    };
    let receipt = recovery::receipt(&id.to_string());
    let result = er_cli::golden_task::run_account_guarded(
        &session,
        &run.config,
        &plan.0.run_at,
        Some(&receipt),
        || {
            if fence.load(Ordering::Acquire) || sigterm.load(Ordering::Acquire) {
                return Err(std::io::Error::other("golden attempt fenced or cancelled").into());
            }
            Ok(())
        },
    )
    .map_err(|e| format!("{e:#}"));
    finished.store(true, Ordering::Release);
    let _ = monitor.join();
    let snapshot = if result.is_ok() {
        crate::duckfleet_reads::page_start(&session, None)
            .ok()
            .map(|(head, _)| head)
    } else {
        None
    };
    let closed = session.close();
    if let Err(error) = &closed {
        // An unacknowledged close leaves worker cleanup to upstream idle and
        // drain bounds. It does not change what was published.
        tracing::warn!(remote_attempt = %id, error = %error, "remote session close was not acknowledged");
    }
    let held = settle(owner, id, &session);
    if let Err(message) = result {
        return Err(stage_error(match held {
            Some(held) => format!("{message}; {held}"),
            None => message,
        }));
    }
    if let Some(held) = held {
        // Published, but the release could not be recorded: say so truthfully.
        tracing::warn!(remote_attempt = %id, detail = %held, "published attempt is still held");
    }
    closed.map_err(|e| stage_error(e.to_string()))?;
    let output = LakeRef::<Golden>::new(snapshot.unwrap_or(-1), 0)
        .serialize_reference()
        .map_err(|_| stage_error("could not serialize golden lake reference".into()))?;
    Ok((
        PodTaskRun {
            skipped: false,
            outcome: "done",
            output,
            counters: None,
        },
        snapshot,
    ))
}

fn memory_bytes(value: &str) -> Option<u64> {
    for (suffix, scale) in [
        ("GiB", 1_073_741_824_u64),
        ("MiB", 1_048_576),
        ("GB", 1_000_000_000),
        ("MB", 1_000_000),
    ] {
        if let Some(number) = value.strip_suffix(suffix) {
            return number.trim().parse::<u64>().ok()?.checked_mul(scale);
        }
    }
    None
}

fn open(
    binding: &er_duckfleet::Binding<'_>,
    options: TaskOptions,
    journal: std::sync::Arc<ChannelJournal>,
) -> Result<Session, er_duckfleet::execution::ExecutionError> {
    // Never compiled into deployed images. The integration harness runs the
    // real exec-task child against its own loopback-only disposable gateway.
    #[cfg(feature = "disposable-proof")]
    if let Ok(address) = std::env::var("ER_DUCKFLEET_PROOF_ADDRESS") {
        let address = address
            .parse()
            .map_err(|_| er_duckfleet::execution::Failure::Parameter)?;
        return Session::open_loopback_task_proof_journaled(
            address,
            binding.bearer_token().to_owned(),
            options,
            journal,
        );
    }
    Session::open_remote_task_journaled(binding, options, journal)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_is_operator_only_and_scoped_to_golden() {
        let policy = |_: &str| Some("/policy.json".to_owned());
        assert!(selected(JobKind::RunAllFull, "golden", |_| None).is_none());
        assert!(selected(JobKind::RunAllFull, "match", policy).is_none());
        assert_eq!(
            selected(JobKind::RunAllIncremental, "golden", policy),
            Some(PathBuf::from("/policy.json"))
        );
        // Same task body, but not qualified outside run_all.
        assert!(selected(JobKind::Correct, "golden", policy).is_none());
        assert!(selected(JobKind::ApplyCorrections, "golden", policy).is_none());
    }

    /// The table is exhaustive over the real DAGs: a new pod task, or an
    /// existing task added to another kind, must be given a route here.
    /// Control nodes run in the control plane and never touch a lake engine.
    #[test]
    fn every_pod_task_has_an_explicit_route() {
        let mut unrouted = Vec::new();
        let mut remote = Vec::new();
        for kind in JobKind::ALL {
            for node in &crate::dags::dag_for(kind).nodes {
                if node.kind != er_dag::NodeKind::Pod {
                    continue;
                }
                match route(kind, node.task_id) {
                    None => unrouted.push(format!("{}/{}", kind.as_str(), node.task_id)),
                    Some(Route::Remote) => {
                        remote.push(format!("{}/{}", kind.as_str(), node.task_id));
                    }
                    Some(Route::Embedded(reason)) => assert_ne!(reason, ""),
                }
            }
        }
        assert!(
            unrouted.is_empty(),
            "pod tasks with no DuckFleet route: {unrouted:?}"
        );
        // Widening remote execution is a decision with evidence, not a side
        // effect of a table edit: change this list only with a qualification.
        assert_eq!(
            remote,
            ["run_all_full/golden", "run_all_incremental/golden"]
        );
    }

    #[test]
    fn a_task_outside_its_kind_has_no_route() {
        assert_eq!(route(JobKind::Extract, "golden"), None);
        assert_eq!(route(JobKind::RunAllFull, "not_a_task"), None);
    }
    #[test]
    fn memory_units_preserve_decimal_and_binary_budgets() {
        assert_eq!(memory_bytes("4GB"), Some(4_000_000_000));
        assert_eq!(memory_bytes("512MiB"), Some(536_870_912));
        assert!(memory_bytes("unlimited").is_none());
        assert!(memory_bytes("18446744073709551615GiB").is_none());
    }
}
