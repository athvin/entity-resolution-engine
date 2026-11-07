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
            // Every kind: this harness is generic, and a test's job must not
            // fail to route because the harness forgot to list its kind. The
            // field goes away with the allowlist in #645.
            dag_kinds: er_server::dags::JobKind::ALL
                .iter()
                .map(|k| k.as_str().to_string())
                .collect(),
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
            pump(&mut self.executor, &mut self.frames).await;
            state = pg::get_job(client, org_id, job_id)
                .await
                .unwrap()
                .map(|j| j.state)
                .unwrap_or_default();
            if state != "running" && state != "dispatching" {
                break;
            }
            tokio::time::sleep(PUMP_EVERY).await;
        }
        state
    }
}
