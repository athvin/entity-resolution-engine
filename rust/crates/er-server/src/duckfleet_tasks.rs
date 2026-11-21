//! Default-off, operator-owned selection for the account golden task.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use er_dag::{DurableOutput, LakeRef, Plan, TaskError, WorkOrder};
use er_duckfleet::execution::{Compute, Session, TaskOptions};
use serde::Deserialize;

use crate::dags::{PodTaskRun, edges::Golden, plans::RunPlan};

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

/// A policy path comes from the task-pod deployment, never job parameters.
pub(crate) fn selected(task: &str, lookup: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    (task == "golden")
        .then(|| lookup("ER_DUCKFLEET_TASK_POLICY"))
        .flatten()
        .map(PathBuf::from)
}

/// Execute without opening an embedded lake alongside the remote writer.
/// Errors follow the same task finish channel as embedded task errors.
pub(crate) fn run(
    order: &WorkOrder,
    attempt: &str,
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
        attempt_id: attempt.to_owned(),
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
    let mut session = open(&binding, options).map_err(|e| stage_error(e.to_string()))?;
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
    let result =
        er_cli::golden_task::run_account_guarded(&session, &run.config, &plan.0.run_at, || {
            if fence.load(Ordering::Acquire) || sigterm.load(Ordering::Acquire) {
                return Err(std::io::Error::other("golden attempt fenced or cancelled").into());
            }
            Ok(())
        })
        .map_err(|e| stage_error(format!("{e:#}")));
    finished.store(true, Ordering::Release);
    let _ = monitor.join();
    let snapshot = if result.is_ok() {
        crate::duckfleet_reads::page_start(&session, None)
            .ok()
            .map(|(head, _)| head)
    } else {
        None
    };
    let closed = session.close().map_err(|e| stage_error(e.to_string()));
    result?;
    closed?;
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
) -> Result<Session, er_duckfleet::execution::ExecutionError> {
    // Never compiled into deployed images. The integration harness runs the
    // real exec-task child against its own loopback-only disposable gateway.
    #[cfg(feature = "disposable-proof")]
    if let Ok(address) = std::env::var("ER_DUCKFLEET_PROOF_ADDRESS") {
        let address = address
            .parse()
            .map_err(|_| er_duckfleet::execution::Failure::Parameter)?;
        return Session::open_loopback_task_proof(
            address,
            binding.bearer_token().to_owned(),
            options,
        );
    }
    Session::open_remote_task(binding, options)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_is_operator_only_and_scoped_to_golden() {
        assert!(selected("golden", |_| None).is_none());
        assert!(selected("match", |_| Some("/policy.json".into())).is_none());
        assert_eq!(
            selected("golden", |_| Some("/policy.json".into())),
            Some(PathBuf::from("/policy.json"))
        );
    }
    #[test]
    fn memory_units_preserve_decimal_and_binary_budgets() {
        assert_eq!(memory_bytes("4GB"), Some(4_000_000_000));
        assert_eq!(memory_bytes("512MiB"), Some(536_870_912));
        assert!(memory_bytes("unlimited").is_none());
        assert!(memory_bytes("18446744073709551615GiB").is_none());
    }
}
