//! Driving a real job through the real executor, for the Postgres tier.
//!
//! Standing a job up on the executor path takes the same forty lines every
//! time: a real control plane for the pods to fetch work orders from, a
//! `FakePodApi` over a temp directory, an `ExecutorConfig`, `connect`, `adopt`,
//! and then a loop that runs the passes and applies the watch frames until the
//! job leaves `running`. `pump` was already copied verbatim into
//! `pg_run_all_pipeline.rs` and `pg_run_all_dag.rs`.
//!
//! It is extracted here because #645 needs it eight more times. The tests that
//! cover correction passes, overlays, mass update, training, provisioning,
//! approvals and entity events currently reach the engine through
//! `dispatcher::drain_and_correct`, `train_and_register` and
//! `dispatch_once_with` — the legacy in-process arm — and that arm is what
//! Phase 4 deletes. Their assertions are about outcomes rather than about the
//! arm, so they move to this path rather than being deleted with it.
//!
//! The ticket named two test files for this (`pg_attempt_reporter.rs` to
//! delete, `pg_job_retry.rs` to rewrite). The real surface is those two plus
//! the eight that call the arm directly.
//!
//! Only `pump` lives here so far. The rest of the harness — serving a control
//! plane, connecting an adopted executor, pumping a job to settlement — lands
//! with the first ported test, and that is blocked: `apply_corrections` cannot
//! apply anything on the task path, because `apply_staged` reads its payloads
//! from `order.params["staged_actions"]` and nothing in the tree ever writes
//! that key. Porting a test onto it would only move the failure.

use er_server::executor::Executor;

/// One scheduler iteration: the passes, then every watch frame that arrived.
pub async fn pump(
    executor: &mut Executor,
    frames: &mut tokio::sync::mpsc::UnboundedReceiver<er_k8s::WatchFrame>,
) {
    executor.passes().await;
    while let Ok(frame) = frames.try_recv() {
        executor.apply_frame(frame).await;
    }
}
