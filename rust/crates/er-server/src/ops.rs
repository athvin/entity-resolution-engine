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
        queued
            .iter()
            .filter_map(|r| r.oldest_created_at)
            .min()
            .map_or(0, |at| {
                elapsed_seconds(chrono::Utc::now().timestamp(), at.timestamp())
            }),
    );
    Ok(())
}

/// Why a managed-pod observation could not be published.
#[derive(Debug, thiserror::Error)]
pub enum PodSnapshotError {
    #[error("reading the managed pods failed")]
    Api(#[from] er_k8s::ApiError),
    #[error("reading the managed pods timed out")]
    Timeout(#[from] tokio::time::error::Elapsed),
    #[error("a Pending pod has no creation timestamp")]
    MissingCreatedAt,
}

fn elapsed_seconds(now: i64, created: i64) -> u64 {
    u64::try_from(now.saturating_sub(created)).unwrap_or_default()
}

fn oldest_pending_age(pods: &[er_k8s::PodSnapshot], now: i64) -> Result<u64, PodSnapshotError> {
    let mut oldest = 0;
    for pod in pods.iter().filter(|p| p.phase == er_k8s::PodPhase::Pending) {
        let created = pod
            .created_at_unix_seconds
            .ok_or(PodSnapshotError::MissingCreatedAt)?;
        oldest = oldest.max(elapsed_seconds(now, created));
    }
    Ok(oldest)
}

/// Publish a current managed-pod sample through the executor's existing API seam.
/// Failed reads leave the last gauge untouched and do not advance its heartbeat.
///
/// # Errors
/// A failed/timed-out LIST, or a Pending pod without its creation timestamp.
pub async fn pod_snapshot(api: &dyn er_k8s::PodApi, now: i64) -> Result<(), PodSnapshotError> {
    let listing = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        api.list(&er_k8s::Selector(crate::executor::MANAGED_SELECTOR.into())),
    )
    .await??;
    let age = oldest_pending_age(&listing.pods, now)?;
    operational::pod_snapshot_succeeded(age);
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
    operational::task_completed(
        class,
        &row.task_id,
        outcome,
        elapsed,
        row.termination_reason.as_deref(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use er_k8s::{
        ApiError, EventSnapshot, JobSnapshot, Listing, ObjectKind, PodApi, PodSnapshot, Selector,
        WatchFrame,
    };
    use futures::{future::BoxFuture, stream::BoxStream};
    use serde_json::json;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };

    fn pod(phase: &str, created: Option<i64>) -> PodSnapshot {
        let timestamp = created.map(|secs| {
            chrono::DateTime::from_timestamp(secs, 0)
                .unwrap()
                .to_rfc3339()
        });
        let raw = serde_json::from_value(json!({
            "metadata": {"name": "synthetic-runner", "creationTimestamp": timestamp},
            "status": {"phase": phase},
        }))
        .unwrap();
        PodSnapshot::from_pod(&raw)
    }

    #[test]
    fn pending_age_uses_kubernetes_time_and_current_phase() {
        let pods = vec![
            pod("Pending", Some(100)),
            pod("Pending", Some(300)),
            pod("Running", Some(0)),
            pod("Succeeded", Some(0)),
        ];
        assert_eq!(pods[0].created_at_unix_seconds, Some(100));
        assert_eq!(oldest_pending_age(&pods, 1000).unwrap(), 900);
        assert_eq!(
            oldest_pending_age(&[pod("Pending", Some(1100))], 1000).unwrap(),
            0,
            "future clock skew cannot wrap into a huge age"
        );
        assert_eq!(oldest_pending_age(&[], 1000).unwrap(), 0);
        assert_eq!(
            oldest_pending_age(&[pod("Running", None)], 1000).unwrap(),
            0
        );
        assert!(matches!(
            oldest_pending_age(&[pod("Pending", None)], 1000),
            Err(PodSnapshotError::MissingCreatedAt)
        ));
    }

    struct Observations {
        listing: Mutex<Listing>,
        fail: AtomicBool,
    }

    impl PodApi for Observations {
        fn list(&self, selector: &Selector) -> BoxFuture<'_, Result<Listing, ApiError>> {
            assert_eq!(selector.0, crate::executor::MANAGED_SELECTOR);
            let response = if self.fail.load(Ordering::SeqCst) {
                Err(ApiError::RateLimited {
                    retry_after_secs: None,
                })
            } else {
                Ok(self.listing.lock().unwrap().clone())
            };
            Box::pin(async move { response })
        }
        fn create_job(
            &self,
            _: &er_k8s::k8s_openapi::api::batch::v1::Job,
        ) -> BoxFuture<'_, Result<(), ApiError>> {
            unreachable!()
        }
        fn get_job(&self, _: &str) -> BoxFuture<'_, Result<Option<JobSnapshot>, ApiError>> {
            unreachable!()
        }
        fn list_events(&self, _: &str) -> BoxFuture<'_, Result<Vec<EventSnapshot>, ApiError>> {
            unreachable!()
        }
        fn patch_labels(
            &self,
            _: ObjectKind,
            _: &str,
            _: &[(&str, Option<&str>)],
        ) -> BoxFuture<'_, Result<(), ApiError>> {
            unreachable!()
        }
        fn delete_job(&self, _: &str) -> BoxFuture<'_, Result<(), ApiError>> {
            unreachable!()
        }
        fn watch(&self, _: &Selector) -> BoxStream<'_, WatchFrame> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn a_failed_or_incomplete_list_never_reports_an_empty_success() {
        let api = Observations {
            listing: Mutex::new(Listing::default()),
            fail: AtomicBool::new(true),
        };
        assert!(matches!(
            pod_snapshot(&api, 1000).await,
            Err(PodSnapshotError::Api(_))
        ));
        api.fail.store(false, Ordering::SeqCst);
        api.listing.lock().unwrap().pods = vec![pod("Pending", None)];
        assert!(matches!(
            pod_snapshot(&api, 1000).await,
            Err(PodSnapshotError::MissingCreatedAt)
        ));
        api.listing.lock().unwrap().pods = vec![pod("Pending", Some(100))];
        pod_snapshot(&api, 1000).await.expect("known current age");
        api.listing.lock().unwrap().pods.clear();
        pod_snapshot(&api, 1000)
            .await
            .expect("healthy empty list replaces the old value");
    }
}
