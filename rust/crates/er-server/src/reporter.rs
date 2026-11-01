//! The attempt REPORTER (batch-orchestration-design §11 Phase 1): the legacy
//! in-process dispatcher plans task rows at claim and drives the same
//! statements the Kubernetes executor will, with a synthetic launch identity
//! (`k8s_job_name = 'inproc'`, `pod_uid` = the dispatcher epoch, `pod_name`
//! NULL) — so the UI shows real stages and real tries before any pod exists.
//!
//! A reporter, not an executor: observation must never change a run. Every
//! method here is best-effort — a reporting failure is logged and the job
//! proceeds exactly as it would have without a ledger — and the reporter holds
//! its OWN Postgres connection so it can apply stage events live while the
//! blocking engine half runs.

use std::collections::BTreeMap;

use serde_json::json;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_postgres::Client;

use er_dag::{NodeKind, PassAction, TaskState};
use er_entities::job::Job;

use crate::dags::{self, JobKind};
use crate::{pg, taskpg};

/// What the engine's [`er_cli::StageObserver`] seam emits across the
/// blocking-thread boundary.
#[derive(Debug)]
pub enum StageEvent {
    Started(String),
    Succeeded(String, Option<i64>),
    Failed(String, String),
}

/// The observer the dispatcher hands the engine: stage boundaries become
/// channel events, applied on the async side by [`AttemptReporter::apply`].
pub struct ChannelObserver(UnboundedSender<StageEvent>);

impl ChannelObserver {
    #[must_use]
    pub fn new(tx: UnboundedSender<StageEvent>) -> Self {
        ChannelObserver(tx)
    }
}

impl er_cli::StageObserver for ChannelObserver {
    fn stage_started(&self, stage: &str) {
        let _ = self.0.send(StageEvent::Started(stage.to_string()));
    }
    fn stage_succeeded(&self, stage: &str, snapshot_end: Option<i64>) {
        let _ = self
            .0
            .send(StageEvent::Succeeded(stage.to_string(), snapshot_end));
    }
    fn stage_failed(&self, stage: &str, detail: &str) {
        let _ = self
            .0
            .send(StageEvent::Failed(stage.to_string(), detail.to_string()));
    }
}

/// One planned job's task rows, as the reporter drives them.
pub struct PlannedRun {
    pub kind: JobKind,
    pub job_id: String,
    pub org_id: String,
    /// The fence every verdict carries (§4.5).
    pub clear_number: i64,
    /// `task_id` → (live row id, node kind).
    rows: BTreeMap<String, (String, NodeKind)>,
}

impl PlannedRun {
    #[must_use]
    pub fn has_task(&self, task_id: &str) -> bool {
        self.rows.contains_key(task_id)
    }
}

/// The synthetic launch identity: there is no Kubernetes Job behind an
/// in-process try, and the name says so.
pub const INPROC: &str = "inproc";

/// The live row id behind a task name, with the miss logged (a vocabulary
/// mismatch between the engine's stage names and the compiled graph).
fn row_id(run: &PlannedRun, task: &str) -> Option<String> {
    let Some((id, _)) = run.rows.get(task) else {
        eprintln!(
            "attempt reporter: job {} has no task row named '{task}'",
            run.job_id
        );
        return None;
    };
    Some(id.clone())
}

/// The dispatcher's ledger half. One per dispatcher process, with its own
/// connection and epoch.
pub struct AttemptReporter {
    client: Client,
    epoch: String,
}

impl AttemptReporter {
    /// # Errors
    /// The reporter's own connection could not be established; the caller runs
    /// without a ledger.
    pub async fn connect(dsn: &str) -> Result<Self, tokio_postgres::Error> {
        let client = pg::connect(dsn).await?;
        let _ = pg::set_actor(&client, "attempt-reporter").await;
        Ok(AttemptReporter {
            client,
            epoch: er_ids::new_v7(),
        })
    }

    /// Plan the claimed job's task rows (one transaction with the claim, §4.5).
    /// `None` — with the reason logged — means the job runs unreported: an
    /// unparseable kind, a structural-fingerprint mismatch on a retry, or a
    /// reporting-path failure. Never fails the job.
    pub async fn plan(&mut self, job: &Job) -> Option<PlannedRun> {
        match self.plan_inner(job).await {
            Ok(planned) => planned,
            Err(e) => {
                eprintln!("attempt reporter: planning job {} failed: {e}", job.id);
                None
            }
        }
    }

    async fn plan_inner(&mut self, job: &Job) -> Result<Option<PlannedRun>, tokio_postgres::Error> {
        let Some(kind) = JobKind::parse(&job.kind) else {
            return Ok(None);
        };
        let dag = dags::dag_for(kind);
        if let Some(fp) = job.structural_fingerprint.as_deref()
            && fp != dag.structural_fingerprint
        {
            eprintln!(
                "attempt reporter: job {} was planned against another build's graph; running \
                 unreported rather than mixing ledgers",
                job.id
            );
            return Ok(None);
        }
        // A legacy retry is a from-scratch re-execution of the whole kind, so
        // its ledger image is a clear of every task: terminal rows rotate,
        // pending rows reset, and the fence advances (§5.4).
        let existing =
            er_entities::task_instance::read::by_job(&self.client, &job.org_id, &job.id).await?;
        if !existing.is_empty() {
            let all: Vec<String> = dag.nodes.iter().map(|n| n.task_id.to_string()).collect();
            match taskpg::clear(&mut self.client, &job.org_id, &job.id, &all, 2, None, None).await {
                Ok(_) => {}
                Err(taskpg::ClearError::Canceling) => return Ok(None),
                Err(taskpg::ClearError::Pg(e)) => return Err(e),
            }
        }
        let planned = taskpg::plan(&mut self.client, job, &dag, &self.epoch, None).await?;
        let Some(planned_job) = planned else {
            // The claim this plan rode was not ours to commit (the job left
            // `dispatching` under us) — run unreported.
            return Ok(None);
        };
        let rows =
            er_entities::task_instance::read::by_job(&self.client, &job.org_id, &job.id).await?;
        let mut map = BTreeMap::new();
        for row in rows.into_iter().filter(|r| r.live == Some(true)) {
            let kind = if row.kind == "control" {
                NodeKind::Control
            } else {
                NodeKind::Pod
            };
            map.insert(row.task_id, (row.id, kind));
        }
        Ok(Some(PlannedRun {
            kind,
            job_id: job.id.clone(),
            org_id: job.org_id.clone(),
            clear_number: planned_job.clear_number.unwrap_or(0),
            rows: map,
        }))
    }

    /// `none → scheduled → queued → running` under the synthetic identity.
    pub async fn pod_started(&mut self, run: &PlannedRun, task: &str) {
        let Some(id) = row_id(run, task) else {
            return;
        };
        if let Err(e) = self.pod_started_inner(run, &id).await {
            eprintln!(
                "attempt reporter: starting {task} on job {} failed: {e}",
                run.job_id
            );
        }
    }

    async fn pod_started_inner(
        &mut self,
        run: &PlannedRun,
        id: &str,
    ) -> Result<(), tokio_postgres::Error> {
        er_entities::task_instance::update::mark_scheduled(
            &self.client,
            &json!({ "inproc": true }),
            id,
            run.clear_number,
        )
        .await?;
        let claimed =
            taskpg::claim_launch(&self.client, &self.epoch, INPROC, None, 1_000_000).await?;
        if let Some(row) = &claimed
            && row.id != id
        {
            eprintln!(
                "attempt reporter: claimed '{}' while reporting '{id}' — two scheduled rows \
                 should be impossible on the in-process path",
                row.id
            );
        }
        er_entities::task_instance::update::start(
            &self.client,
            None,
            &self.epoch,
            None,
            None,
            id,
            INPROC,
        )
        .await?;
        Ok(())
    }

    pub async fn pod_succeeded(&mut self, run: &PlannedRun, task: &str, snapshot_end: Option<i64>) {
        let Some(id) = row_id(run, task) else {
            return;
        };
        if let Err(e) = er_entities::task_instance::update::finish(
            &self.client,
            false,
            "done",
            None,
            None,
            snapshot_end,
            None,
            None,
            &id,
            &self.epoch,
        )
        .await
        {
            eprintln!(
                "attempt reporter: finishing {task} on job {} failed: {e}",
                run.job_id
            );
        }
    }

    /// Fail a task row whatever state it is in: a `none` row is walked to
    /// `queued` first (the archive guard takes `queued | running`).
    pub async fn pod_failed(&mut self, run: &PlannedRun, task: &str, detail: &str) {
        let Some(id) = row_id(run, task) else {
            return;
        };
        // Best-effort walk; each statement no-ops (zero rows) past its state.
        let _ = er_entities::task_instance::update::mark_scheduled(
            &self.client,
            &json!({ "inproc": true }),
            &id,
            run.clear_number,
        )
        .await;
        let _ = taskpg::claim_launch(&self.client, &self.epoch, INPROC, None, 1_000_000).await;
        let class = pg::FailureClass::classify(detail);
        let failure = taskpg::TaskFailure {
            terminal_state: "failed",
            // The legacy JOB-level retry governs; a task-level retry here would
            // retry twice. A requeued job clears and replans its rows instead.
            retryable: false,
            escalate: false,
            exit_code: None,
            termination_reason: None,
            error_class: Some(class.as_str()),
            error_detail: Some(detail),
            k8s_job_name: Some(INPROC),
            backoff: taskpg::POD_BACKOFF,
        };
        if let Err(e) = taskpg::fail_or_retry_task(&mut self.client, &id, &failure).await {
            eprintln!(
                "attempt reporter: failing {task} on job {} failed: {e}",
                run.job_id
            );
        }
    }

    /// `none → scheduled → running` through the control claim (§3.6).
    pub async fn control_started(&mut self, run: &PlannedRun, task: &str) {
        let Some(id) = row_id(run, task) else {
            return;
        };
        let _ = er_entities::task_instance::update::mark_scheduled(
            &self.client,
            &json!({ "inproc": true }),
            &id,
            run.clear_number,
        )
        .await;
        if let Err(e) =
            er_entities::task_instance::update::claim_control(&self.client, &self.epoch).await
        {
            eprintln!(
                "attempt reporter: control claim for {task} on job {} failed: {e}",
                run.job_id
            );
        }
    }

    pub async fn control_succeeded(
        &mut self,
        run: &PlannedRun,
        task: &str,
        output: Option<serde_json::Value>,
    ) {
        let Some(id) = row_id(run, task) else {
            return;
        };
        if let Err(e) = er_entities::task_instance::update::finish_control(
            &self.client,
            false,
            "done",
            output.as_ref(),
            None,
            &id,
            &self.epoch,
        )
        .await
        {
            eprintln!(
                "attempt reporter: finishing control {task} on job {} failed: {e}",
                run.job_id
            );
        }
    }

    pub async fn control_failed(&mut self, run: &PlannedRun, task: &str, detail: &str) {
        let Some(id) = row_id(run, task) else {
            return;
        };
        let _ = er_entities::task_instance::update::mark_scheduled(
            &self.client,
            &json!({ "inproc": true }),
            &id,
            run.clear_number,
        )
        .await;
        let _ = er_entities::task_instance::update::claim_control(&self.client, &self.epoch).await;
        let class = pg::FailureClass::classify(detail);
        let failure = taskpg::TaskFailure {
            terminal_state: "failed",
            retryable: false,
            escalate: false,
            exit_code: None,
            termination_reason: None,
            error_class: Some(class.as_str()),
            error_detail: Some(detail),
            k8s_job_name: None,
            backoff: taskpg::CONTROL_BACKOFF,
        };
        if let Err(e) = taskpg::fail_or_retry_task(&mut self.client, &id, &failure).await {
            eprintln!(
                "attempt reporter: failing control {task} on job {} failed: {e}",
                run.job_id
            );
        }
    }

    /// Apply one engine stage event; every engine stage is a pod task.
    pub async fn apply(&mut self, run: &PlannedRun, event: StageEvent) {
        match event {
            StageEvent::Started(stage) => self.pod_started(run, &stage).await,
            StageEvent::Succeeded(stage, snapshot_end) => {
                self.pod_succeeded(run, &stage, snapshot_end).await;
            }
            StageEvent::Failed(stage, detail) => self.pod_failed(run, &stage, &detail).await,
        }
    }

    /// Settle the ledger once the job's outcome is decided: the readiness
    /// evaluator's propagations land (`upstream_failed` and kin), and any row
    /// whose rule would FIRE on a stopped run — the teardown `finish` the
    /// legacy arm never executes — records `canceled`. The JOB row stays the
    /// legacy arm's to settle.
    pub async fn settle(&mut self, run: &PlannedRun, job_succeeded: bool) {
        if let Err(e) = self.settle_inner(run, job_succeeded).await {
            eprintln!(
                "attempt reporter: settling job {}'s rows failed: {e}",
                run.job_id
            );
        }
    }

    async fn settle_inner(
        &mut self,
        run: &PlannedRun,
        job_succeeded: bool,
    ) -> Result<(), tokio_postgres::Error> {
        let dag = dags::dag_for(run.kind);
        let rows = er_entities::task_instance::read::by_job(&self.client, &run.org_id, &run.job_id)
            .await?;
        let states: Vec<TaskState> = dag
            .nodes
            .iter()
            .map(|n| {
                rows.iter()
                    .find(|r| r.task_id == n.task_id && r.live == Some(true))
                    .and_then(|r| TaskState::parse(&r.state))
                    .unwrap_or(TaskState::None)
            })
            .collect();
        for (idx, action) in er_dag::pass(&dag, &states) {
            let task_id = dag.nodes[idx].task_id;
            let Some((row_id, _)) = run.rows.get(task_id) else {
                continue;
            };
            let state = match action {
                PassAction::Propagate(state) => state,
                PassAction::Schedule if !job_succeeded => {
                    // The rule would fire (finish is all_terminal), but the
                    // legacy arm has no teardown execution: the run stopped and
                    // this task never ran.
                    TaskState::Canceled
                }
                PassAction::Schedule => {
                    eprintln!(
                        "attempt reporter: job {} succeeded with '{task_id}' still schedulable — \
                         a stage the engine never reported",
                        run.job_id
                    );
                    TaskState::Canceled
                }
            };
            er_entities::task_instance::update::propagate(
                &self.client,
                state.as_str(),
                row_id,
                run.clear_number,
            )
            .await?;
        }
        Ok(())
    }
}

/// The reporter and its planned run, threaded through the dispatcher's arms as
/// one reborrowable handle.
pub struct RunReport<'a> {
    pub reporter: &'a mut AttemptReporter,
    pub run: &'a PlannedRun,
}

impl RunReport<'_> {
    pub async fn apply(&mut self, event: StageEvent) {
        self.reporter.apply(self.run, event).await;
    }
    #[must_use]
    pub fn has_task(&self, task: &str) -> bool {
        self.run.has_task(task)
    }
    pub async fn pod_started(&mut self, task: &str) {
        self.reporter.pod_started(self.run, task).await;
    }
    pub async fn pod_succeeded(&mut self, task: &str, snapshot_end: Option<i64>) {
        self.reporter
            .pod_succeeded(self.run, task, snapshot_end)
            .await;
    }
    pub async fn pod_failed(&mut self, task: &str, detail: &str) {
        self.reporter.pod_failed(self.run, task, detail).await;
    }
    pub async fn control_started(&mut self, task: &str) {
        self.reporter.control_started(self.run, task).await;
    }
    pub async fn control_succeeded(&mut self, task: &str, output: Option<serde_json::Value>) {
        self.reporter
            .control_succeeded(self.run, task, output)
            .await;
    }
    pub async fn control_failed(&mut self, task: &str, detail: &str) {
        self.reporter.control_failed(self.run, task, detail).await;
    }
    pub async fn settle(&mut self, job_succeeded: bool) {
        self.reporter.settle(self.run, job_succeeded).await;
    }
}

/// Run a blocking engine half while applying its stage events live: the
/// channel's sender goes into the closure (handed to the engine's observer),
/// and the receiver drains on this side until the blocking half drops it.
///
/// # Errors
/// The blocking half's own error, or its panic as text — the same contract as
/// the bare `spawn_blocking` it replaces.
pub async fn observe_blocking<T>(
    report: Option<&mut RunReport<'_>>,
    spawn: impl FnOnce(
        Option<UnboundedSender<StageEvent>>,
    ) -> tokio::task::JoinHandle<Result<T, String>>,
) -> Result<T, String> {
    let (tx, rx) = unbounded_channel();
    if let Some(rr) = report {
        let handle = spawn(Some(tx));
        let (joined, ()) = tokio::join!(handle, drain_events(rr, rx));
        joined.unwrap_or_else(|e| Err(format!("dispatcher task panicked: {e}")))
    } else {
        drop(tx);
        spawn(None)
            .await
            .unwrap_or_else(|e| Err(format!("dispatcher task panicked: {e}")))
    }
}

/// Drain one blocking half's stage events into the ledger (shared by
/// `observe_blocking` and `drain_and_correct`'s hand-rolled join).
pub(crate) async fn drain_events(rr: &mut RunReport<'_>, mut rx: UnboundedReceiver<StageEvent>) {
    while let Some(event) = rx.recv().await {
        rr.apply(event).await;
    }
}
