//! Operational observations from committed control-plane state.
//!
//! Queue snapshots run independently of job traffic so an empty queue emits zero.
//! Failed reads emit no replacement values: the snapshot heartbeat alarm makes
//! stale data visible. Transition counters are emitted after guarded writes; a
//! duplicate/stale terminal report therefore cannot manufacture another outcome.

use er_entities::task_instance::TaskInstance;
use er_obs::operational::{self, Class, Outcome};

/// Publish a complete queue/concurrency sample without tenant dimensions.
///
/// # Errors
/// Returns a failed database read; the dispatcher logs it and retries next tick.
pub async fn snapshot(client: &tokio_postgres::Client) -> Result<(), tokio_postgres::Error> {
    let queued = er_entities::job::read::operational_queue(client).await?;
    let active = er_entities::task_instance::read::operational_active(client).await?;
    for class in Class::ALL {
        let depth = queued
            .iter()
            .filter(|r| Class::from_value(r.resource_class.as_deref()) == class)
            .map(|r| u64::try_from(r.depth.unwrap_or_default()).unwrap_or_default())
            .sum();
        let count = active
            .iter()
            .filter(|r| Class::from_value(r.resource_class.as_deref()) == class)
            .map(|r| u64::try_from(r.active.unwrap_or_default()).unwrap_or_default())
            .sum();
        operational::queue_snapshot(class, depth, count);
    }
    operational::snapshot_succeeded(
        queued
            .iter()
            .map(|r| u64::try_from(r.depth.unwrap_or_default()).unwrap_or_default())
            .sum(),
    );
    Ok(())
}

/// Record the returned row of a committed terminal write, never a pre-write row.
pub fn task_completed(row: &TaskInstance) {
    if row.kind != "pod" {
        return;
    }
    let outcome = match row.state.as_str() {
        "failed" | "timed_out" => Outcome::Failed,
        "skipped" => Outcome::Skipped,
        "succeeded" if row.outcome.as_deref() == Some("no_op") => Outcome::NoOp,
        "succeeded" => Outcome::Done,
        _ => return,
    };
    let elapsed = row
        .started_at
        .zip(row.finished_at)
        .and_then(|(start, end)| (end - start).to_std().ok());
    let class = Class::from_value(row.resource_class.as_deref());
    operational::task_completed(class, &row.task_id, outcome, elapsed);
    operational::pod_failed(class, row.termination_reason.as_deref());
}
