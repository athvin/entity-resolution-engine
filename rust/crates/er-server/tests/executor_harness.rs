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
//! `pg_drain_dispatcher.rs` is the first ported test, and porting it is what
//! found #760 — `apply_staged` read its payloads from a params key nothing
//! wrote, so a correction pass applied nothing and settled `succeeded`.

use std::path::PathBuf;
use std::sync::Arc;

use er_server::executor::{Executor, ExecutorConfig};
use er_server::{AppState, pg};

use crate::fake_pod_api::FakePodApi;

/// How many pump iterations a job gets before the caller gives up. Generous:
/// every pod pays a cold DuckDB open, and a full pipeline is seven of them.
const MAX_PUMPS: usize = 900;
/// The gap between pumps. Long enough not to spin, short enough that a
/// single-task kind settles in well under a second of wall clock.
const PUMP_EVERY: std::time::Duration = std::time::Duration::from_millis(200);

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

/// A live executor over a real control plane, with the fake pod API's frames.
pub struct Harness {
    pub executor: Executor,
    pub frames: tokio::sync::mpsc::UnboundedReceiver<er_k8s::WatchFrame>,
}

impl Harness {
    /// Serve the control plane on an ephemeral port, point a `FakePodApi` at
    /// it, and connect an adopted executor.
    ///
    /// `data_root` is the directory the fake pods run against — the same
    /// temporary tree the test wrote the tenant's lake and drop files into.
    pub async fn start(dsn: &str, data_root: PathBuf) -> Harness {
        Harness::start_with_profile(dsn, data_root, Ok(None)).await
    }

    /// [`start`] with the provisioning profile handed in.
    ///
    /// The seam a `provision` test needs: `create_catalog` takes its profile
    /// from `ExecutorConfig` rather than the environment, because a test here
    /// cannot set an environment variable at all — `std::env::set_var` is
    /// `unsafe` in edition 2024 and the workspace denies `unsafe_code`. This is
    /// the executor-path equivalent of `dispatch_once_with`'s profile
    /// argument, which exists for exactly that reason.
    pub async fn start_with_profile(
        dsn: &str,
        data_root: PathBuf,
        profile: Result<Option<er_server::provision::Profile>, er_server::provision::ProfileError>,
    ) -> Harness {
        // The REAL control plane, so a pod fetches a real work order over HTTP
        // rather than one the test assembled. A mis-built order fails here,
        // which is the whole point of running the tier this way.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = er_server::app(AppState::with_pg("op-secret".to_string(), dsn.to_string()));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        // Not retained on the struct: nothing has needed to `script` a
        // failure through the harness yet, and a field no caller reads is a
        // field that rots. It comes back with its first user.
        let (api, frames) = FakePodApi::new(format!("http://{addr}"), data_root);
        let api = Arc::new(api);
        let cfg = ExecutorConfig {
            profile,
            // No per-kind allowlist to opt into: the executor owns every kind.
            in_flight_cap: 2,
            template: er_k8s::manifest::REFERENCE_TEMPLATE.to_string(),
            image_digest: None,
        };
        let mut executor = Executor::connect(dsn, Arc::clone(&api) as Arc<dyn er_k8s::PodApi>, cfg)
            .await
            .unwrap();
        executor.adopt().await;
        Harness { executor, frames }
    }

    /// Pump until the job leaves `running`/`dispatching`, and return the state
    /// it settled in.
    ///
    /// Returns whatever the row says when the budget runs out, so a caller's
    /// assertion reports the stuck state rather than a timeout with no detail.
    pub async fn run_to_settlement(
        &mut self,
        client: &tokio_postgres::Client,
        org_id: &str,
        job_id: &str,
    ) -> String {
        let mut state = String::new();
        for _ in 0..MAX_PUMPS {
            // Claim on every iteration, which is what `dispatcher::run_loop`
            // does. Claiming ONCE was wrong whenever the queue held more than
            // the job under test: `pg::claim` takes the oldest queued row for
            // an org with no active job, so a test whose earlier phase left an
            // `apply_corrections` behind had that claimed instead, and its own
            // job sat `queued` while the pumps ran. The legacy arm's tests
            // asserted only that SOMETHING was claimed, so they never saw it.
            //
            // A claim failure is not fatal: once every job is terminal there
            // is nothing to claim and `dispatch_once` returns false.
            let _ = er_server::dispatcher::dispatch_once(client, Some(&mut self.executor))
                .await
                .expect("dispatch_once");
            pump(&mut self.executor, &mut self.frames).await;
            state = pg::get_job(client, org_id, job_id)
                .await
                .unwrap()
                .map(|j| j.state)
                .unwrap_or_default();
            if state != "running" && state != "dispatching" && state != "queued" {
                break;
            }
            tokio::time::sleep(PUMP_EVERY).await;
        }
        state
    }

    /// Enqueue an `apply_corrections` job for the dataset and run it to
    /// settlement, asserting it succeeded.
    ///
    /// This is the one-line stand-in for a `dispatcher::drain_and_correct`
    /// call: a correction pass as the product actually runs one, through the
    /// executor. The panic names every task row, because a pass that settles
    /// without doing its work is the failure mode this path has already had
    /// once (#760) and "succeeded" alone never explains it.
    pub async fn apply_corrections(
        &mut self,
        client: &tokio_postgres::Client,
        org_id: &str,
        dataset_id: &str,
    ) {
        let _ = self
            .run_kind(client, org_id, dataset_id, "apply_corrections")
            .await;
    }

    /// Claim and run an ALREADY-enqueued job to settlement, returning the
    /// state it settled in.
    ///
    /// The stand-in for a `dispatcher::dispatch_once_with` call where the test
    /// enqueued the job itself and wants to assert on the settled row rather
    /// than have the harness assert for it.
    pub async fn run_queued(
        &mut self,
        client: &tokio_postgres::Client,
        org_id: &str,
        job_id: &str,
    ) -> String {
        self.run_to_settlement(client, org_id, job_id).await
    }

    /// Enqueue a job of `kind` for the dataset, run it to settlement
    /// asserting it succeeded, and return its JOB ID.
    ///
    /// The id matters to callers: on this path the run id IS the job id, so
    /// anything a kind stamps with its run — a model version label, a
    /// provenance field — is named after a value only `enqueue` knows. The
    /// legacy arm let a test choose that string, which is why some of them
    /// assert against a hard-coded one.
    pub async fn run_kind(
        &mut self,
        client: &tokio_postgres::Client,
        org_id: &str,
        dataset_id: &str,
        kind: &str,
    ) -> String {
        let queued = pg::enqueue(
            client,
            org_id,
            Some(dataset_id),
            kind,
            0,
            None,
            None,
            pg::Provenance::default(),
        )
        .await
        .unwrap_or_else(|e| panic!("enqueue {kind}: {e}"));
        assert!(
            er_server::dispatcher::dispatch_once(client, Some(&mut self.executor))
                .await
                .expect("dispatch_once"),
            "the executor claims and plans the {kind} job"
        );
        let settled = self.run_to_settlement(client, org_id, &queued.id).await;
        if settled != "succeeded" {
            let rows = er_entities::task_instance::read::by_job(client, org_id, &queued.id)
                .await
                .expect("task rows");
            let shape: Vec<(String, String, Option<String>)> = rows
                .iter()
                .map(|r| (r.task_id.clone(), r.state.clone(), r.error_detail.clone()))
                .collect();
            panic!("the {kind} job settled {settled:?}: {shape:?}");
        }
        queued.id
    }
}
