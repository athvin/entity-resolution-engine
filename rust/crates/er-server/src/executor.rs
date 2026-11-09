//! The Kubernetes executor's dispatcher half (batch-orchestration-design §5.1,
//! §6.5, §6.6): planning every claimed job, the readiness pass, the
//! launcher's write-ahead claim → create, the cancel/zombie/stuck-pending
//! sweeps, the watch's verdict application, and adoption on start-up and
//! resync. Kubernetes owns placement, deadlines and cleanup; Postgres stays
//! the state of record; this module is the translation layer between
//! `er_k8s`'s pure verdicts and the §4.5 statements.
//!
//! Every job kind is the executor's. There is no `ER_EXECUTOR_DAG_KINDS`
//! allowlist and no in-process arm to fall back to (#645), so a kind the DAG
//! registry cannot parse is refused terminally at planning rather than routed
//! elsewhere. The fallback per-attempt token (hash at rest, nulled at finish)
//! is the live credential.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

use serde_json::{Value, json};
use tokio_postgres::Client;

use er_dag::{PassAction, SettleVerdict, TaskState};
use er_entities::job::Job;
use er_entities::task_instance::TaskInstance;
use er_k8s::{
    JobSnapshot, ObjectKind, PodApi, PodSnapshot, PreStartVerdict, RowState, RowView, Selector,
    TerminalState, TerminalVerdict, WatchFrame,
};

use crate::dags::{self, JobKind};
use crate::{pg, taskpg, webhook};

/// The watch/list selector every managed object carries (§6.5).
pub const MANAGED_SELECTOR: &str = "er.dupezero.com/managed=task,er.dupezero.com/done!=true";
/// The zombie bound (§5.7).
const ZOMBIE_STALE_SECS: i64 = 300;
/// The capacity wait (§4.5; the chart's `executor.quotaWaitSeconds`).
const QUOTA_WAIT_SECS: i64 = 30;

/// Configuration, from the chart through the environment.
pub struct ExecutorConfig {
    /// The provisioning profile `create_catalog` needs, resolved once here
    /// rather than read from the environment inside the task.
    ///
    /// This is the seam `dispatch_once_with` has and the executor path lacked.
    /// A Postgres-tier test cannot set an environment variable —
    /// `std::env::set_var` is `unsafe` in edition 2024 and the workspace denies
    /// `unsafe_code` — so a task calling `Profile::from_env()` itself cannot be
    /// driven by a test at all, which is why provisioning had no coverage on
    /// the path that runs it.
    pub profile: Result<Option<crate::provision::Profile>, crate::provision::ProfileError>,
    pub in_flight_cap: i64,
    /// The Job template (the chart's ConfigMap, mounted as a file).
    pub template: String,
    pub image_digest: Option<String>,
    /// Orgs enrolled in the chart/Terraform tenant-runner registry. All other
    /// orgs use the unprivileged runner account, never another org's identity.
    pub runner_orgs: BTreeSet<String>,
}

impl ExecutorConfig {
    /// `None` only when a configured task template cannot be read.
    ///
    /// The executor used to be opt-in per kind through `ER_EXECUTOR_DAG_KINDS`
    /// (§11's rollout allowlist), returning `None` whenever that variable was
    /// absent or empty. The allowlist is gone with the legacy in-process arm
    /// (#645): there is no second path for a kind to fall back to, so every
    /// kind the DAG registry parses is the executor's, and one it does not
    /// parse is refused terminally by `plan_claimed` rather than falling
    /// through. What still disables the executor is the absence of a namespace
    /// or an in-cluster identity, which is what keeps a host dispatcher out of
    /// the business of creating Jobs — see [`from_env`].
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let in_flight_cap = std::env::var("ER_EXECUTOR_IN_FLIGHT_CAP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        let template = match std::env::var("ER_TASK_TEMPLATE_PATH") {
            Ok(path) => match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    tracing::error!(path = %path, error = %e, "cannot read the task template");
                    return None;
                }
            },
            Err(_) => er_k8s::manifest::REFERENCE_TEMPLATE.to_string(),
        };
        let runner_orgs =
            match parse_runner_orgs(std::env::var("ER_TENANT_RUNNER_ORGS").ok().as_deref()) {
                Ok(orgs) => orgs,
                Err(message) => {
                    tracing::error!(error = message, "invalid tenant runner configuration");
                    return None;
                }
            };
        Some(ExecutorConfig {
            runner_orgs,
            profile: crate::provision::Profile::from_env(),
            in_flight_cap,
            template,
            image_digest: std::env::var("ER_IMAGE_DIGEST")
                .ok()
                .filter(|v| !v.is_empty()),
        })
    }
}

/// Build the executor and its watch task from the environment, when
/// configured: a namespace and an in-cluster Kubernetes identity. Either one
/// missing disables the executor with the reason logged.
///
/// On a host — local development — `KubeApi::in_cluster` is what fails, so the
/// dispatcher runs without an executor and falls through to the in-process arm
/// in `dispatcher.rs`. That fallback is what #645 deletes; when it goes, a
/// dispatcher with no executor must stop claiming rather than mark a row
/// `running` it cannot advance, and `make run` and `scripts/frontend-dev/up.sh`
/// keep API-only work. That is the decision recorded against Phase 4 in
/// `docs/batch-orchestration-design.md` (#638).
pub async fn from_env(
    dsn: &str,
) -> (
    Option<Executor>,
    Option<tokio::sync::mpsc::UnboundedReceiver<WatchFrame>>,
) {
    let Some(cfg) = ExecutorConfig::from_env() else {
        return (None, None);
    };
    let namespace = match std::env::var("ER_NAMESPACE") {
        Ok(ns) if !ns.is_empty() => ns,
        _ => {
            match std::fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/namespace")
            {
                Ok(ns) => ns.trim().to_string(),
                Err(e) => {
                    tracing::warn!(error = %e, "the executor is disabled: no namespace (set ER_NAMESPACE)");
                    return (None, None);
                }
            }
        }
    };
    let api: Arc<dyn PodApi> = match er_k8s::KubeApi::in_cluster(&namespace) {
        Ok(api) => Arc::new(api),
        Err(e) => {
            tracing::warn!(error = %e, "the executor is disabled: no in-cluster Kubernetes identity");
            return (None, None);
        }
    };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    {
        let api = Arc::clone(&api);
        tokio::spawn(async move {
            er_k8s::observer::run_watch(api.as_ref(), Selector(MANAGED_SELECTOR.into()), tx).await;
        });
    }
    match Executor::connect(dsn, api, cfg).await {
        Ok(ex) => (Some(ex), Some(rx)),
        Err(e) => {
            tracing::warn!(error = %crate::errfmt::chain(&e), "the executor is disabled: connecting to the control plane failed");
            (None, None)
        }
    }
}

/// The executor: its own Postgres connection (statement composition needs
/// `&mut Client`), the PodApi, and its process epoch.
pub struct Executor {
    client: Client,
    api: Arc<dyn PodApi>,
    cfg: ExecutorConfig,
    /// Outbound mail for the run report the settle sends. Built here rather
    /// than handed in so `connect`'s signature stays what the integration
    /// tests already call; with no SES configuration this is the `off` mailer
    /// and `send` is a no-op.
    mailer: crate::mail::Mailer,
    epoch: String,
    /// Per-launch pre-start clocks (first observation of the standing
    /// evidence); an executor restart re-arms the bounds, which is safe — a
    /// bound fires later, never earlier.
    pending_since: BTreeMap<String, Instant>,
}

impl Executor {
    /// # Errors
    /// The executor's own connection could not be established.
    pub async fn connect(
        dsn: &str,
        api: Arc<dyn PodApi>,
        cfg: ExecutorConfig,
    ) -> Result<Self, er_pg_tls::Error> {
        let client = pg::connect(dsn).await?;
        let _ = pg::set_actor(&client, "executor").await;
        Ok(Executor {
            client,
            api,
            cfg,
            mailer: crate::mail::Mailer::from_env().await,
            epoch: er_ids::new_v7(),
            pending_since: BTreeMap::new(),
        })
    }

    /// Adoption (§6.6): the new process owns every live row; the watch's first
    /// `Resync` frame then reconciles against the listing.
    pub async fn adopt(&mut self) {
        if let Err(e) = er_entities::job::update::adopt(&self.client, &self.epoch).await {
            tracing::warn!(error = %e, "adopting jobs failed");
        }
        if let Err(e) = er_entities::task_instance::update::adopt(&self.client, &self.epoch).await {
            tracing::warn!(error = %e, "adopting rows failed");
        }
    }

    /// Plan a freshly-claimed job (§4.5's planning transaction). Refusals are
    /// terminal in the same breath — including a kind the registry cannot
    /// parse, which is why no allowlist has to be consulted first.
    pub async fn plan_claimed(&mut self, job: &Job) {
        let Some(kind) = JobKind::parse(&job.kind) else {
            let _ = taskpg::refuse_plan(
                &self.client,
                &job.id,
                "config",
                &format!("kind '{}' does not parse into the DAG registry", job.kind),
            )
            .await;
            return;
        };
        let dag = dags::dag_for(kind);
        if let Some(fp) = job.structural_fingerprint.as_deref()
            && fp != dag.structural_fingerprint
        {
            let _ = taskpg::refuse_plan(
                &self.client,
                &job.id,
                "config",
                &format!(
                    "the job was planned against structural fingerprint {fp}; this build compiles \
                     {} — clear into a new job rather than mixing graphs (§9.4)",
                    dag.structural_fingerprint
                ),
            )
            .await;
            return;
        }
        match taskpg::plan(
            &mut self.client,
            job,
            &dag,
            &self.epoch,
            self.cfg.image_digest.as_deref(),
        )
        .await
        {
            Ok(Some(_)) => {}
            Ok(None) => tracing::warn!(er.job_id = %job.id, "the job left dispatching mid-plan"),
            Err(e) => tracing::error!(er.job_id = %job.id, error = %e, "planning the job failed"),
        }
    }

    /// One scheduler iteration (§5.1's loop steps, between the tick and the
    /// drain): cancel sweep, readiness, launcher, zombie and stuck-pending
    /// sweeps. Each failure is logged, never propagated — the loop re-runs.
    pub async fn passes(&mut self) -> bool {
        let mut worked = false;
        for (name, result) in [
            ("cancel sweep", self.cancel_sweep().await),
            ("readiness pass", self.readiness_pass().await),
            ("control pass", self.control_pass().await),
            ("launcher", self.launch_pass().await),
            ("zombie sweep", self.zombie_sweep().await),
            ("stuck-pending sweep", self.stuck_pending_sweep().await),
        ] {
            match result {
                Ok(did) => worked |= did,
                Err(e) => tracing::warn!(er.sweep = %name, error = %e, "a sweep failed"),
            }
        }
        worked
    }

    // ---- readiness + settle -------------------------------------------------

    async fn dag_jobs(&self, state: &str) -> Result<Vec<Job>, tokio_postgres::Error> {
        let jobs = er_entities::job::read::list(&self.client, None, Some(state), 200).await?;
        // Every kind the registry parses; one it does not cannot have been
        // planned, so it has no task rows for these passes to act on.
        Ok(jobs
            .into_iter()
            .filter(|j| JobKind::parse(&j.kind).is_some())
            .collect())
    }

    async fn readiness_pass(&mut self) -> Result<bool, tokio_postgres::Error> {
        let mut worked = false;
        let mut jobs = self.dag_jobs("running").await?;
        jobs.extend(self.dag_jobs("canceling").await?);
        for job in jobs {
            match self.readiness_for(&job).await {
                Ok(did) => worked |= did,
                Err(e) => {
                    tracing::warn!(er.job_id = %job.id, error = %e, "the readiness check failed");
                }
            }
        }
        Ok(worked)
    }

    #[expect(clippy::too_many_lines, reason = "one transaction, §5.3's whole pass")]
    async fn readiness_for(&mut self, job: &Job) -> Result<bool, tokio_postgres::Error> {
        let Some(kind) = JobKind::parse(&job.kind) else {
            return Ok(false);
        };
        let dag = dags::dag_for(kind);
        let tx = self.client.transaction().await?;
        let Some(locked) = er_entities::job::read::lock(&tx, &job.id)
            .await?
            .into_iter()
            .next()
        else {
            return Ok(false);
        };
        let clear_number = locked.clear_number.unwrap_or(0);
        let rows = er_entities::task_instance::read::by_job(&tx, &job.org_id, &job.id).await?;
        let live: BTreeMap<&str, &TaskInstance> = rows
            .iter()
            .filter(|r| r.live == Some(true))
            .map(|r| (r.task_id.as_str(), r))
            .collect();
        let states: Vec<TaskState> = dag
            .nodes
            .iter()
            .map(|n| {
                live.get(n.task_id)
                    .and_then(|r| TaskState::parse(&r.state))
                    .unwrap_or(TaskState::None)
            })
            .collect();
        let actions = er_dag::pass(&dag, &states);
        let mut effective = states.clone();
        let mut worked = !actions.is_empty();
        for (idx, action) in actions {
            let node = &dag.nodes[idx];
            let Some(row) = live.get(node.task_id) else {
                continue;
            };
            match action {
                PassAction::Schedule => {
                    let input = compose_input(&dag, idx, &live, job);
                    let updated = er_entities::task_instance::update::mark_scheduled(
                        &tx,
                        &input,
                        &row.id,
                        clear_number,
                    )
                    .await?;
                    if updated.is_empty() {
                        // A zero-row verdict rolls the whole pass back (§4.5).
                        return Ok(false);
                    }
                    // Settle reads the pass's OWN verdicts: a `none` left in
                    // `effective` here would read as the deadlock branch.
                    effective[idx] = TaskState::Scheduled;
                }
                PassAction::Propagate(state) => {
                    let updated = er_entities::task_instance::update::propagate(
                        &tx,
                        state.as_str(),
                        &row.id,
                        clear_number,
                    )
                    .await?;
                    if updated.is_empty() {
                        return Ok(false);
                    }
                    effective[idx] = state;
                }
            }
        }
        // Settle is the last step of the same pass, reading its own verdicts.
        let settled = match er_dag::settle(&dag, &effective) {
            SettleVerdict::NotYet => None,
            SettleVerdict::Settled(outcome) => {
                let (class, detail) = failure_text(&dag, &effective, &live);
                let state = match outcome {
                    TaskState::Failed => "failed",
                    TaskState::Canceled => "canceled",
                    _ => "succeeded",
                };
                let rows = er_entities::job::update::settle(
                    &tx,
                    state,
                    class.as_deref(),
                    detail.as_deref(),
                    &job.id,
                )
                .await?;
                rows.into_iter().next()
            }
            SettleVerdict::Deadlock(idx) => {
                let rows = er_entities::job::update::settle(
                    &tx,
                    "failed",
                    Some("internal"),
                    Some(&format!(
                        "task '{}' is pending with every upstream terminal — a scheduler bug \
                         (Airflow's deadlock branch)",
                        dag.nodes[idx].task_id
                    )),
                    &job.id,
                )
                .await?;
                rows.into_iter().next()
            }
        };
        tx.commit().await?;
        if let Some(settled) = settled {
            worked = true;
            tracing::info!(
                er.job_id = %settled.id,
                er.job_kind = %settled.kind,
                er.state = %settled.state,
                "the job settled"
            );
            let _ = webhook::emit_job_event(&self.client, &settled).await;
            // The entity-event aggregate (O3c-2), which the legacy arm emitted
            // from `run_job`'s tail. It is NOT part of `emit_job_event`: that
            // sends one `job.*` per outcome, while this sends at most one
            // `entity.*` per kind.
            //
            // The counts come from `finish`'s durable output rather than from a
            // lake read here, because this process cannot open a lake. Reading
            // them off the row map costs nothing — the readiness pass already
            // loaded it.
            //
            // Absent for every kind with no `finish` task, and that is correct:
            // `lake_maintain`, `train` and `profile` reconcile no entities, so
            // there is nothing to announce.
            let pairs: Vec<(String, i64)> = live
                .get("finish")
                .and_then(|row| row.output.as_ref())
                .and_then(|out| out.get("doc"))
                .and_then(|doc| doc.get("entity_counts"))
                .and_then(|v| {
                    serde_json::from_value::<Vec<crate::dags::plans::EntityCount>>(v.clone()).ok()
                })
                .unwrap_or_default()
                .into_iter()
                .map(|c| (c.event_type, c.n))
                .collect();
            if !pairs.is_empty() {
                crate::dispatcher::emit_entity_aggregates(&self.client, &settled, &pairs).await;
            }
            self.send_run_report(&settled, &pairs).await;
        }
        Ok(worked)
    }

    /// The run report for a settled job, one message per configured recipient.
    ///
    /// This rides the settle last, after the outcome webhook and the entity
    /// aggregate, because a subscriber watching all three should see the
    /// outcome first.
    ///
    /// It belongs here rather than only in the legacy arm, and that was a real
    /// gap: `run_report_messages` had exactly one caller, `dispatch_claimed`,
    /// while the deploy routed every kind to this executor. So the report had
    /// already stopped being sent in dev and in production — nothing reported
    /// it, because a mail that is not sent looks identical to a tenant who
    /// never configured one. Deleting the arm (#645) would have made that
    /// permanent and silent.
    ///
    /// Best-effort throughout, exactly as it was: the job's outcome is already
    /// persisted, and reporting an outcome is not part of producing it. A
    /// tenant with no `ER_RUN_REPORT_TO`, or a kind no steward waits on, sends
    /// nothing — `run_report_messages` owns those rules and is unit-tested
    /// without a mailer.
    async fn send_run_report(&self, settled: &Job, entity_counts: &[(String, i64)]) {
        // The org is read for its name and `env`; a job whose org has vanished
        // simply sends nothing.
        let org = match pg::get_org_by_id(&self.client, &settled.org_id).await {
            Ok(Some(org)) => org,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(er.job_id = %settled.id, error = %e, "the run report was skipped: reading the org failed");
                return;
            }
        };
        for message in
            crate::dispatcher::run_report_messages(&org.name, &org.env, settled, entity_counts)
        {
            if let Err(e) = self.mailer.send(&message).await {
                tracing::warn!(
                    er.job_id = %settled.id,
                    error = %e,
                    // `message.to` is deliberately NOT a field: it is a
                    // recipient address, and §10.4 keeps plaintext email off
                    // anything exportable.
                    "sending the run report failed"
                );
            }
        }
    }

    // ---- control tasks ------------------------------------------------------

    /// Claim and run every runnable CONTROL task (§3.6).
    ///
    /// In-process, not in a pod, and that is the whole distinction: a control
    /// body's work IS the control plane — a registry row, an org state change
    /// — so it runs where the credential already is. A pod could not do it
    /// without holding a database credential, which §6.4 forbids.
    ///
    /// Claim → run → finish, one at a time: these are short Postgres writes,
    /// so there is no capacity argument for concurrency, and running them
    /// serially keeps the failure story simple.
    async fn control_pass(&mut self) -> Result<bool, tokio_postgres::Error> {
        let mut worked = false;
        loop {
            let claimed =
                er_entities::task_instance::update::claim_control(&self.client, &self.epoch)
                    .await?;
            let Some(row) = claimed.into_iter().next() else {
                return Ok(worked);
            };
            worked = true;
            self.run_one_control(&row).await;
        }
    }

    async fn run_one_control(&mut self, row: &TaskInstance) {
        let Some(kind) = JobKind::parse_row_kind(&self.client, row).await else {
            let _ = taskpg::fail_or_retry_task(
                &mut self.client,
                &row.id,
                &taskpg::TaskFailure {
                    terminal_state: "failed",
                    retryable: false,
                    escalate: false,
                    exit_code: None,
                    termination_reason: Some("config"),
                    error_class: Some("config"),
                    error_detail: Some("the job's kind does not parse into the DAG registry"),
                    k8s_job_name: None,
                    backoff: taskpg::POD_BACKOFF,
                },
            )
            .await;
            return;
        };
        // The tenant env and params a control body may need: `stage_approved`
        // reads the published config's approval tag, `create_catalog` reads
        // the provisioning plan. Reading a YAML file is not opening a lake, so
        // this does not make a control task a pod task (§3.5 draws that line
        // at the lake, and `control_tasks_no_engine.rs` enforces it).
        let (lake_env, drop_root, params) = match pg::get_org_by_id(&self.client, &row.org_id).await
        {
            Ok(Some(org)) => {
                let pair = pg::lake_env_for_job(&self.client, &org, row.dataset_id.as_deref())
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or((Value::Null, None));
                let params = pg::get_job(&self.client, &row.org_id, &row.job_id)
                    .await
                    .ok()
                    .flatten()
                    .map_or(Value::Null, |j| j.params);
                (pair.0, pair.1, params)
            }
            _ => (Value::Null, None, Value::Null),
        };
        // The order a control body reads: the same shape a pod's work order
        // has, built here rather than fetched over HTTP — this half IS the
        // control plane, so there is nothing to fetch from.
        let order = er_dag::WorkOrder {
            org_id: row.org_id.clone(),
            dataset_id: row.dataset_id.clone(),
            job_id: row.job_id.clone(),
            run_id: row.job_id.clone(),
            task_id: row.task_id.clone(),
            kind: kind.as_str().to_string(),
            try_number: row.try_number,
            resource_class: row.resource_class.clone(),
            segment: row.segment.clone(),
            input: row.input.clone().unwrap_or(Value::Null),
            params,
            lake_env,
            drop_root,
            models: Value::Null,
            labels: Value::Null,
            // A control body never holds the credential; `advance_sf_cursor`
            // works from the pod's report and its own Postgres client.
            connection: Value::Null,
        };
        let mut ctx = er_dag::ControlCtx {
            order: &order,
            client: &self.client,
        };
        match dags::run_control_task(kind, &row.task_id, &mut ctx, &self.cfg.profile).await {
            Ok(run) => {
                let _ = er_entities::task_instance::update::finish_control(
                    &self.client,
                    run.skipped,
                    run.outcome,
                    Some(&run.output),
                    run.counters.as_ref(),
                    &row.id,
                    &self.epoch,
                )
                .await;
            }
            Err(e) => {
                let failure = taskpg::control_failure(&e);
                let _ = taskpg::fail_or_retry_task(&mut self.client, &row.id, &failure).await;
            }
        }
    }

    // ---- the launcher -------------------------------------------------------

    async fn launch_pass(&mut self) -> Result<bool, tokio_postgres::Error> {
        let mut worked = false;
        loop {
            // The fallback credential: minted per launch, hash at rest on the
            // row, nulled at finish (§6.4's recorded fallback).
            let token = mint_token();
            let claimed = taskpg::claim_launch(
                &self.client,
                &self.epoch,
                None,
                Some(&pg::hash_secret(&token)),
                self.cfg.in_flight_cap,
            )
            .await?;
            let Some(row) = claimed else {
                return Ok(worked);
            };
            worked = true;
            // Airflow's stale-workload re-read before every create (§4.5).
            let fresh = er_entities::task_instance::read::retry_lineage(&self.client, &row.id)
                .await?
                .into_iter()
                .next();
            let fresh_ok = fresh.as_ref().is_some_and(|f| {
                f.state == "queued"
                    && f.try_number == row.try_number
                    && f.owner_epoch.as_deref() == Some(self.epoch.as_str())
            });
            if !fresh_ok {
                continue;
            }
            if let Err(e) = self.create_for(&row, &token).await {
                tracing::error!(er.task_instance_id = %row.id, error = %e, "creating the Kubernetes Job failed");
            }
        }
    }

    async fn create_for(&mut self, row: &TaskInstance, token: &str) -> Result<(), String> {
        let job_name = row
            .k8s_job_name
            .clone()
            .ok_or("claim_launch always writes the name")?;
        let Some(kind) = JobKind::parse_row_kind(&self.client, row).await else {
            return Err(format!("row {} has no parseable job kind", row.id));
        };
        let dag = dags::dag_for(kind);
        let timeout = dag
            .node_by_task_id(&row.task_id)
            .and_then(|(_, n)| n.policy.timeout_secs)
            .unwrap_or(class_default_timeout(row.resource_class.as_deref()));
        let attempt = er_k8s::RenderAttempt {
            job_name: job_name.clone(),
            task_instance_id: row.id.clone(),
            task_id: row.task_id.clone(),
            job_id: row.job_id.clone(),
            try_number: row.try_number,
            resource_class: row.resource_class.clone().unwrap_or_else(|| "M".into()),
            owner_epoch: self.epoch.clone(),
            structural_fingerprint: dag.structural_fingerprint.clone(),
            policy_hash: dag.policy_hash.clone(),
            active_deadline_seconds: i64::try_from(timeout).unwrap_or(21_600),
            through_task: row.task_id.clone(),
            // Already carried on the job/task rows "so a deploy mid-run is
            // legible"; this puts it on the Job object too, which is where it
            // is legible WITHOUT a database. Empty rather than absent when the
            // install pins a tag: the template's placeholder must always
            // substitute, because any surviving `${ER_` is a launch-time
            // refusal.
            image_digest: self.cfg.image_digest.clone().unwrap_or_default(),
        };
        let mut manifest = er_k8s::render_job(&self.cfg.template, &attempt)
            .map_err(|e| format!("render: {}", crate::errfmt::chain(&e)))?;
        // The claimed task row is authoritative. Neither the request params
        // nor the tenant's lake env may choose a Kubernetes/AWS identity.
        let service_account = runner_service_account(&row.org_id, &self.cfg.runner_orgs);
        inject_attempt_env(&mut manifest, &job_name, token, &service_account);
        match self.api.create_job(&manifest).await {
            Ok(()) => {
                self.pending_since.insert(job_name, Instant::now());
                Ok(())
            }
            Err(er_k8s::ApiError::Quota { message }) => {
                self.capacity_wait(row, &job_name, "quota", &message).await;
                Ok(())
            }
            Err(er_k8s::ApiError::AlreadyExists) => {
                // §6.5's create-outcome probe: ours → adopt; anything else →
                // never reuse the name.
                match self.api.get_job(&job_name).await {
                    Ok(Some(found))
                        if found
                            .annotations
                            .get(er_k8s::manifest::TASK_INSTANCE_ANNOTATION)
                            .is_some_and(|id| id == &row.id)
                            && found
                                .annotations
                                .get(er_k8s::manifest::FINGERPRINT_ANNOTATION)
                                .is_some_and(|fp| fp == &dag.structural_fingerprint)
                            && !found.deletion_timestamp
                            && !found.labels.contains_key(er_k8s::manifest::DONE_LABEL) =>
                    {
                        let _ = self
                            .api
                            .patch_labels(
                                ObjectKind::Job,
                                &job_name,
                                &[(er_k8s::manifest::OWNER_LABEL, Some(&self.epoch))],
                            )
                            .await;
                        Ok(())
                    }
                    _ => {
                        self.requeue(row, &job_name, "foreign_job_name", QUOTA_WAIT_SECS)
                            .await;
                        Ok(())
                    }
                }
            }
            Err(er_k8s::ApiError::RateLimited { retry_after_secs }) => {
                let wait = retry_after_secs
                    .and_then(|s| i64::try_from(s).ok())
                    .unwrap_or(QUOTA_WAIT_SECS);
                self.requeue(row, &job_name, "rate_limited", wait).await;
                Ok(())
            }
            Err(e) => {
                // A transport error may have persisted the Job: probe by name.
                if let Ok(Some(_)) = self.api.get_job(&job_name).await {
                    Ok(())
                } else {
                    self.requeue(row, &job_name, "create_failed", QUOTA_WAIT_SECS)
                        .await;
                    Err(format!("create: {}", crate::errfmt::chain(&e)))
                }
            }
        }
    }

    // ---- sweeps -------------------------------------------------------------

    async fn cancel_sweep(&mut self) -> Result<bool, tokio_postgres::Error> {
        let jobs = self.dag_jobs("canceling").await?;
        let mut worked = false;
        for job in jobs {
            worked = true;
            let pending = er_entities::task_instance::update::cancel_pending(
                &self.client,
                &job.org_id,
                &job.id,
            )
            .await?;
            let live =
                er_entities::task_instance::update::cancel_live(&self.client, &job.org_id, &job.id)
                    .await?;
            let _ = pending;
            for row in live {
                // Control rows only. The `inproc` exemption beside this one
                // existed for the legacy arm's synthetic launch identity;
                // nothing writes that name now (#645).
                if row.kind == "control" {
                    // No pod to wait for: settle at once.
                    let _ = er_entities::task_instance::update::settle_canceled(
                        &self.client,
                        None,
                        Some("canceled"),
                        &row.id,
                        row.k8s_job_name.as_deref(),
                    )
                    .await?;
                } else if let Some(name) = row.k8s_job_name.as_deref() {
                    self.revoke_and_delete(name).await;
                }
            }
            // Rows already canceling from an earlier sweep whose pods are gone
            // settle through the watch; the sweep itself stays idempotent.
        }
        Ok(worked)
    }

    async fn zombie_sweep(&mut self) -> Result<bool, tokio_postgres::Error> {
        let stale = er_entities::task_instance::read::stale_heartbeats(
            &self.client,
            &self.epoch,
            ZOMBIE_STALE_SECS,
        )
        .await?;
        if stale.is_empty() {
            return Ok(false);
        }
        // Stand down while the platform is ending the pod: its evidence lands
        // on the launch budget, not the try budget (§5.7).
        let listing = match self.api.list(&Selector(MANAGED_SELECTOR.into())).await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, "the zombie sweep could not list pods");
                return Ok(false);
            }
        };
        for row in stale {
            let pod = listing
                .pods
                .iter()
                .find(|p| Some(p.name.as_str()) == row.pod_name.as_deref());
            if pod.is_some_and(|p| p.node_not_ready || p.disruption_target.is_some()) {
                continue;
            }
            // The write-ahead: the reason lands with no state change, then the
            // Job is revoked; the watch charges the try once the pod is gone.
            if let Some(pod_uid) = row.pod_uid.as_deref() {
                let _ = er_entities::task_instance::update::note_interruption(
                    &self.client,
                    "heartbeat_timeout",
                    None,
                    &row.id,
                    pod_uid,
                )
                .await;
            }
            if let Some(name) = row.k8s_job_name.as_deref() {
                self.revoke_and_delete(name).await;
            }
        }
        Ok(true)
    }

    async fn stuck_pending_sweep(&mut self) -> Result<bool, tokio_postgres::Error> {
        let rows = er_entities::task_instance::read::live_claimed_pods(&self.client).await?;
        let queued: Vec<&TaskInstance> = rows.iter().filter(|r| r.state == "queued").collect();
        if queued.is_empty() {
            return Ok(false);
        }
        let listing = match self.api.list(&Selector(MANAGED_SELECTOR.into())).await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, "the stuck-pending sweep could not list");
                return Ok(false);
            }
        };
        let mut worked = false;
        for row in queued {
            let Some(job_name) = row.k8s_job_name.as_deref() else {
                continue;
            };
            let job = listing.jobs.iter().find(|j| j.name == job_name);
            let pod = listing
                .pods
                .iter()
                .find(|p| p.name.starts_with(job_name) && p.phase == er_k8s::PodPhase::Pending);
            let age = self
                .pending_since
                .get(job_name)
                .map_or(0, |t| t.elapsed().as_secs());
            let events;
            let newest_failed_create = if job.is_some_and(|j| j.active == 0) && pod.is_none() {
                events = self.api.list_events(job_name).await.unwrap_or_default();
                events
                    .iter()
                    .rev()
                    .find(|e| e.reason == "FailedCreate")
                    .map(|e| e.message.clone())
            } else {
                None
            };
            let verdict = er_k8s::pre_start(
                pod,
                &job.cloned().unwrap_or_default(),
                newest_failed_create.as_deref(),
                age,
                age,
                age,
            );
            match verdict {
                PreStartVerdict::Wait => {}
                PreStartVerdict::RequeueLaunch { reason, detail } => {
                    worked = true;
                    self.api_delete(job_name).await;
                    let reason =
                        detail.map_or_else(|| reason.clone(), |d| format!("{reason}: {d}"));
                    self.requeue(row, job_name, &reason, QUOTA_WAIT_SECS).await;
                }
                PreStartVerdict::CapacityWait { reason, detail } => {
                    worked = true;
                    self.api_delete(job_name).await;
                    let text = detail.unwrap_or_else(|| reason.clone());
                    self.capacity_wait(row, job_name, &reason, &text).await;
                }
            }
        }
        Ok(worked)
    }

    // ---- the watch ----------------------------------------------------------

    /// Apply one watch frame — the §6.5 terminal rules through `er_k8s`'s pure
    /// classifier, plus the zombie write-ahead override.
    pub async fn apply_frame(&mut self, frame: WatchFrame) {
        match frame {
            WatchFrame::Resync => {
                if let Err(e) = self.reconcile().await {
                    tracing::warn!(error = %e, "the resync reconcile failed");
                }
            }
            WatchFrame::Pod(pod) => self.apply_pod(&pod, false).await,
            WatchFrame::PodDeleted(pod) => self.apply_pod(&pod, true).await,
            WatchFrame::Job(job) | WatchFrame::JobDeleted(job) => self.apply_job(&job).await,
        }
    }

    async fn row_for_annotation(
        &self,
        annotations: &BTreeMap<String, String>,
    ) -> Option<TaskInstance> {
        let id = annotations.get(er_k8s::manifest::TASK_INSTANCE_ANNOTATION)?;
        er_entities::task_instance::read::retry_lineage(&self.client, id)
            .await
            .ok()?
            .into_iter()
            .next()
    }

    async fn apply_job(&mut self, job: &JobSnapshot) {
        // The Job stream exists for what a pods-only watch cannot see: the
        // deadline condition, and a Job that never got a pod (§6.5).
        if !job.failed_deadline {
            return;
        }
        let Some(row) = self.row_for_annotation(&job.annotations).await else {
            return;
        };
        if row.k8s_job_name.as_deref() != Some(job.name.as_str()) {
            return; // a stale launch's Job: tombstone only
        }
        let Some(view) = row_view(&row) else { return };
        let verdict = er_k8s::terminal(view, None, job, false, 0);
        self.apply_verdict(&row, job.name.as_str(), verdict).await;
    }

    async fn apply_pod(&mut self, pod: &PodSnapshot, deleted: bool) {
        let Some(row) = self.row_for_annotation(&pod.annotations).await else {
            return;
        };
        let Some(job_name) = row.k8s_job_name.clone() else {
            return;
        };
        if !pod.name.starts_with(job_name.as_str()) {
            return; // §6.5: a pod whose row carries another launch — stale
        }
        if pod.phase == er_k8s::PodPhase::Running {
            self.pending_since.remove(&job_name);
        }
        let Some(view) = row_view(&row) else {
            // The row already holds a verdict: evidence annotates.
            if row.state == "succeeded" || row.state == "skipped" {
                let _ = er_entities::task_instance::update::record_evidence(
                    &self.client,
                    pod.terminated_exit_code.map(i64::from),
                    pod.terminated_reason.as_deref(),
                    &row.id,
                    row.pod_uid.as_deref().unwrap_or(&pod.uid),
                    &job_name,
                )
                .await;
                self.tombstone(&job_name, Some(&pod.name)).await;
            }
            return;
        };
        let job = self
            .api
            .get_job(&job_name)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        let mut verdict = er_k8s::terminal(view, Some(pod), &job, deleted, NO_RECORD_GRACE);
        // The zombie write-ahead: a revoked pod whose row says heartbeat_timeout
        // charges the TRY as infrastructure, not the launch (§5.7).
        if matches!(verdict, TerminalVerdict::Requeue { .. })
            && row.termination_reason.as_deref() == Some("heartbeat_timeout")
        {
            verdict = TerminalVerdict::ApplyRecord(er_k8s::TerminalRecord {
                v: 1,
                attempt: row.id.clone(),
                state: TerminalState::Failed,
                exit: pod.terminated_exit_code.unwrap_or(1),
                error_class: Some("infrastructure".into()),
                termination_reason: Some("heartbeat_timeout".into()),
                detail: Some(
                    "no heartbeat for 5 minutes; something ran and the outcome is unknown".into(),
                ),
                output: None,
                snapshot_start: None,
                snapshot_end: None,
                counters: None,
            });
        }
        self.apply_verdict(&row, &job_name, verdict).await;
    }

    #[expect(clippy::too_many_lines, reason = "one arm per §6.5 verdict")]
    async fn apply_verdict(
        &mut self,
        row: &TaskInstance,
        job_name: &str,
        verdict: TerminalVerdict,
    ) {
        let kind = JobKind::parse_row_kind(&self.client, row).await;
        match verdict {
            TerminalVerdict::Wait => {}
            TerminalVerdict::RecordEvidence { exit_code, reason } => {
                let _ = er_entities::task_instance::update::record_evidence(
                    &self.client,
                    exit_code,
                    reason.as_deref(),
                    &row.id,
                    row.pod_uid.as_deref().unwrap_or_default(),
                    job_name,
                )
                .await;
                self.tombstone(job_name, None).await;
            }
            TerminalVerdict::ApplyRecord(record) => {
                match record.state {
                    TerminalState::Succeeded | TerminalState::Skipped => {
                        // The pod's own finish normally landed this; the record
                        // is the recovery path (a lost report) — same guarded
                        // statement, a no-op when already posted.
                        let _ = er_entities::task_instance::update::finish(
                            &self.client,
                            record.state == TerminalState::Skipped,
                            "done",
                            record.output.as_ref(),
                            record.snapshot_start,
                            record.snapshot_end,
                            record.counters.as_ref(),
                            Some(i64::from(record.exit)),
                            &row.id,
                            row.pod_uid.as_deref().unwrap_or_default(),
                        )
                        .await;
                    }
                    TerminalState::Failed => {
                        let class = record.error_class.as_deref().unwrap_or("internal");
                        let retryable = matches!(
                            class,
                            "transient_io" | "lock_conflict" | "infrastructure" | "resources"
                        );
                        let escalate = class == "resources" && escalates(kind, &row.task_id);
                        let failure = taskpg::TaskFailure {
                            terminal_state: "failed",
                            retryable,
                            escalate,
                            exit_code: Some(i64::from(record.exit)),
                            termination_reason: record.termination_reason.as_deref(),
                            error_class: Some(class),
                            error_detail: record.detail.as_deref(),
                            k8s_job_name: Some(job_name),
                            backoff: taskpg::POD_BACKOFF,
                        };
                        let _ =
                            taskpg::fail_or_retry_task(&mut self.client, &row.id, &failure).await;
                    }
                    TerminalState::Interrupted => { /* classify already deferred */ }
                }
                self.tombstone(job_name, None).await;
            }
            TerminalVerdict::FailNoRecord => {
                let failure = taskpg::TaskFailure {
                    terminal_state: "failed",
                    retryable: false,
                    escalate: false,
                    exit_code: None,
                    termination_reason: Some("no_terminal_record"),
                    error_class: Some("internal"),
                    error_detail: Some(
                        "the pod reached a terminal phase with no terminal record; never a \
                         success and never retryable (§6.5)",
                    ),
                    k8s_job_name: Some(job_name),
                    backoff: taskpg::POD_BACKOFF,
                };
                let _ = taskpg::fail_or_retry_task(&mut self.client, &row.id, &failure).await;
                self.tombstone(job_name, None).await;
            }
            TerminalVerdict::TimedOut => {
                let failure = taskpg::TaskFailure {
                    terminal_state: "timed_out",
                    retryable: false,
                    escalate: false,
                    exit_code: None,
                    termination_reason: Some("DeadlineExceeded"),
                    error_class: Some("timeout"),
                    error_detail: Some(
                        "the Job's activeDeadlineSeconds fired; a longer timeout is a policy \
                         change the operator applies by clearing (§5.5)",
                    ),
                    k8s_job_name: Some(job_name),
                    backoff: taskpg::POD_BACKOFF,
                };
                let _ = taskpg::fail_or_retry_task(&mut self.client, &row.id, &failure).await;
                self.tombstone(job_name, None).await;
            }
            TerminalVerdict::FailResources { reason, detail } => {
                let escalate =
                    escalates(kind, &row.task_id) && row.resource_class.as_deref() != Some("L");
                let failure = taskpg::TaskFailure {
                    terminal_state: "failed",
                    retryable: true,
                    escalate,
                    exit_code: None,
                    termination_reason: Some(&reason),
                    error_class: Some("resources"),
                    error_detail: detail.as_deref(),
                    k8s_job_name: Some(job_name),
                    backoff: taskpg::POD_BACKOFF,
                };
                let _ = taskpg::fail_or_retry_task(&mut self.client, &row.id, &failure).await;
                self.tombstone(job_name, None).await;
            }
            TerminalVerdict::Requeue { reason } => {
                self.api_delete(job_name).await;
                self.requeue(row, job_name, &reason, QUOTA_WAIT_SECS).await;
            }
            TerminalVerdict::SettleCanceled { exit_code, reason } => {
                let _ = er_entities::task_instance::update::settle_canceled(
                    &self.client,
                    exit_code,
                    reason.as_deref(),
                    &row.id,
                    Some(job_name),
                )
                .await;
                self.tombstone(job_name, None).await;
            }
            TerminalVerdict::CompleteRestart { exit_code, reason } => {
                let rotated = er_entities::task_instance::update::archive_restarted(
                    &self.client,
                    exit_code,
                    reason.as_deref(),
                    &row.id,
                    Some(job_name),
                )
                .await;
                if let Ok(rows) = rotated
                    && !rows.is_empty()
                {
                    let _ = er_entities::task_instance::create::insert_cleared_try(
                        &self.client,
                        None,
                        2,
                        self.cfg.image_digest.as_deref(),
                        &row.id,
                    )
                    .await;
                }
                self.tombstone(job_name, None).await;
            }
        }
    }

    // ---- adoption / resync --------------------------------------------------

    async fn reconcile(&mut self) -> Result<(), tokio_postgres::Error> {
        let rows = er_entities::task_instance::read::live_claimed_pods(&self.client).await?;
        let listing = match self.api.list(&Selector(MANAGED_SELECTOR.into())).await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, "the reconcile could not list");
                return Ok(());
            }
        };
        let row_names: BTreeSet<&str> = rows
            .iter()
            .filter_map(|r| r.k8s_job_name.as_deref())
            .collect();
        for row in &rows {
            let Some(job_name) = row.k8s_job_name.as_deref() else {
                continue;
            };
            let found = listing.jobs.iter().find(|j| j.name == job_name);
            match (row.state.as_str(), found) {
                ("restarting", None) => {
                    let rotated = er_entities::task_instance::update::archive_restarted(
                        &self.client,
                        None,
                        Some("no_pod"),
                        &row.id,
                        Some(job_name),
                    )
                    .await;
                    if let Ok(rotated) = rotated
                        && !rotated.is_empty()
                    {
                        let _ = er_entities::task_instance::create::insert_cleared_try(
                            &self.client,
                            None,
                            2,
                            self.cfg.image_digest.as_deref(),
                            &row.id,
                        )
                        .await;
                    }
                }
                ("canceling", None) => {
                    let _ = er_entities::task_instance::update::settle_canceled(
                        &self.client,
                        None,
                        Some("vanished"),
                        &row.id,
                        Some(job_name),
                    )
                    .await;
                }
                ("canceling", Some(_)) => self.revoke_and_delete(job_name).await,
                ("queued" | "running", Some(job)) => {
                    let ours_fp = JobKind::parse_row_kind(&self.client, row)
                        .await
                        .map(|k| dags::dag_for(k).structural_fingerprint);
                    let theirs = job
                        .annotations
                        .get(er_k8s::manifest::FINGERPRINT_ANNOTATION);
                    if ours_fp.as_ref() == theirs {
                        // Ours: adopt — never resubmit (a duplicate attempt is
                        // two lake writers).
                        let _ = self
                            .api
                            .patch_labels(
                                ObjectKind::Job,
                                job_name,
                                &[(er_k8s::manifest::OWNER_LABEL, Some(&self.epoch))],
                            )
                            .await;
                    } else {
                        // Build skew (§6.6): tombstoned, never attributed.
                        self.tombstone(job_name, None).await;
                        let failure = taskpg::TaskFailure {
                            terminal_state: "failed",
                            retryable: false,
                            escalate: false,
                            exit_code: None,
                            termination_reason: Some("build_skew"),
                            error_class: Some("internal"),
                            error_detail: Some(
                                "the running Job was launched from a different build's graph",
                            ),
                            k8s_job_name: Some(job_name),
                            backoff: taskpg::POD_BACKOFF,
                        };
                        let _ =
                            taskpg::fail_or_retry_task(&mut self.client, &row.id, &failure).await;
                    }
                }
                ("running", None) => {
                    // The Job is gone with the row still running: relaunch on
                    // the infrastructure budget; the ledger probe (Phase 3)
                    // reports committed work instead of re-running it.
                    self.requeue(row, job_name, "vanished", QUOTA_WAIT_SECS)
                        .await;
                }
                ("queued", None) => {
                    // The write-ahead says a launch was intended; the fallback
                    // token's value died with the old epoch, so relaunch fresh
                    // rather than recreating under a credential nobody holds.
                    // (The projected-token path removes this divergence from
                    // §6.6's same-name recreate.)
                    self.requeue(row, job_name, "adoption_relaunch", 0).await;
                }
                _ => {}
            }
        }
        // Jobs with no live claimed row: a finished attempt whose delete was
        // lost — tombstone and let TTL take it.
        for job in &listing.jobs {
            if !row_names.contains(job.name.as_str())
                && !job.labels.contains_key(er_k8s::manifest::DONE_LABEL)
            {
                self.tombstone(&job.name, None).await;
            }
        }
        Ok(())
    }

    // ---- helpers ------------------------------------------------------------

    async fn requeue(&mut self, row: &TaskInstance, job_name: &str, reason: &str, wait: i64) {
        let requeued = er_entities::task_instance::update::requeue_launch(
            &self.client,
            wait,
            Some(reason),
            &row.id,
            job_name,
        )
        .await;
        match requeued {
            Ok(rows) if rows.is_empty() => {
                // Stale, or the launch budget is spent: re-read and settle.
                let fresh = er_entities::task_instance::read::retry_lineage(&self.client, &row.id)
                    .await
                    .ok()
                    .and_then(|v| v.into_iter().next());
                if let Some(fresh) = fresh
                    && fresh.live == Some(true)
                    && matches!(fresh.state.as_str(), "queued" | "running")
                    && fresh.launches >= fresh.max_launches
                {
                    let failure = taskpg::TaskFailure {
                        terminal_state: "failed",
                        retryable: true,
                        escalate: false,
                        exit_code: None,
                        termination_reason: Some(reason),
                        error_class: Some("infrastructure"),
                        error_detail: Some(
                            "the infrastructure budget is spent: three launches never ran user \
                             code (§5.5)",
                        ),
                        k8s_job_name: Some(job_name),
                        backoff: taskpg::POD_BACKOFF,
                    };
                    let _ = taskpg::fail_or_retry_task(&mut self.client, &row.id, &failure).await;
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(er.task_instance_id = %row.id, error = %e, "requeueing the row failed");
            }
        }
        self.pending_since.remove(job_name);
    }

    async fn capacity_wait(
        &mut self,
        row: &TaskInstance,
        job_name: &str,
        reason: &str,
        detail: &str,
    ) {
        // The delete comes first; the wait is recorded only when the Job is
        // gone, so a recreate before the delete completed 409s into the launch
        // budget (§4.5).
        self.api_delete(job_name).await;
        let _ = er_entities::task_instance::update::capacity_wait(
            &self.client,
            QUOTA_WAIT_SECS,
            Some(&format!("{reason}: {detail}")),
            &row.id,
            job_name,
        )
        .await;
        self.pending_since.remove(job_name);
        tracing::info!(er.task_instance_id = %row.id, reason = %reason, "waiting on capacity");
    }

    async fn revoke_and_delete(&self, job_name: &str) {
        // dagr's two-step: patch-then-delete, so the deletion is legible as
        // ours (§6.6).
        let _ = self
            .api
            .patch_labels(
                ObjectKind::Job,
                job_name,
                &[(
                    er_k8s::manifest::OWNER_LABEL,
                    Some(er_k8s::manifest::REVOKED),
                )],
            )
            .await;
        self.api_delete(job_name).await;
    }

    async fn api_delete(&self, job_name: &str) {
        if let Err(e) = self.api.delete_job(job_name).await {
            tracing::warn!(er.k8s_job = %job_name, error = %e, "deleting the Kubernetes Job failed");
        }
    }

    async fn tombstone(&self, job_name: &str, pod_name: Option<&str>) {
        // After any outcome is consumed, mark done BEFORE relying on TTL, so a
        // finished attempt is never attributed twice even if the delete is
        // lost (§6.6).
        let _ = self
            .api
            .patch_labels(
                ObjectKind::Job,
                job_name,
                &[(er_k8s::manifest::DONE_LABEL, Some("true"))],
            )
            .await;
        if let Some(pod) = pod_name {
            let _ = self
                .api
                .patch_labels(
                    ObjectKind::Pod,
                    pod,
                    &[(er_k8s::manifest::DONE_LABEL, Some("true"))],
                )
                .await;
        }
        self.api_delete(job_name).await;
    }
}

/// How long the watch tolerates a terminal pod with no record before
/// classifying without one — passed as the caller's clock so the classifier
/// stays pure. Phase 2 applies frames as they arrive and once per loop, so the
/// bound is approximated by the loop cadence.
const NO_RECORD_GRACE: u64 = er_k8s::NO_RECORD_BOUND_SECS;

/// The failure class/detail a settle carries, from the first failure-like
/// leaf's own row.
fn failure_text(
    dag: &er_dag::Dag,
    states: &[TaskState],
    live: &BTreeMap<&str, &TaskInstance>,
) -> (Option<String>, Option<String>) {
    for (idx, state) in states.iter().enumerate() {
        if state.class() == er_dag::StateClass::FailureLike
            && let Some(row) = live.get(dag.nodes[idx].task_id)
        {
            return (
                row.error_class.clone().or_else(|| Some("internal".into())),
                row.error_detail
                    .clone()
                    .or_else(|| Some(format!("task '{}' {}", row.task_id, row.state))),
            );
        }
    }
    (None, None)
}

fn row_view(row: &TaskInstance) -> Option<RowView> {
    let state = match row.state.as_str() {
        "queued" => RowState::Queued,
        "running" => RowState::Running,
        "restarting" => RowState::Restarting,
        "canceling" => RowState::Canceling,
        "succeeded" | "skipped" => {
            return Some(RowView {
                state: RowState::Running,
                finish_landed: true,
            });
        }
        _ => return None,
    };
    Some(RowView {
        state,
        finish_landed: false,
    })
}

/// Does this task escalate class on a resources failure (§5.5)?
fn escalates(kind: Option<JobKind>, task_id: &str) -> bool {
    kind.map(dags::dag_for)
        .and_then(|dag| {
            dag.node_by_task_id(task_id)
                .map(|(_, n)| n.policy.escalate_on_oom)
        })
        .unwrap_or(false)
}

/// The class-default `activeDeadlineSeconds` (infrastructure.md §6.2's sketch).
fn class_default_timeout(class: Option<&str>) -> u64 {
    match class {
        Some("S") => 2 * 3600,
        Some("L") => 24 * 3600,
        _ => 6 * 3600,
    }
}

/// Compose a scheduled node's work order: the positional upstream pins, the
/// job's params, and the leaf verdicts for a teardown (§4.5's `input`).
fn compose_input(
    dag: &er_dag::Dag,
    idx: usize,
    live: &BTreeMap<&str, &TaskInstance>,
    job: &Job,
) -> Value {
    let node = &dag.nodes[idx];
    let pins: Vec<Value> = node
        .upstream
        .iter()
        .map(|u| {
            live.get(dag.nodes[u.index()].task_id)
                .and_then(|r| r.output.clone())
                .unwrap_or(Value::Null)
        })
        .collect();
    let mut input = json!({ "pins": pins, "params": job.params });
    if node.teardown {
        let leaves: BTreeMap<&str, &str> = dag
            .nodes
            .iter()
            .filter(|n| !n.teardown)
            .filter_map(|n| live.get(n.task_id).map(|r| (n.task_id, r.state.as_str())))
            .collect();
        input["leaves"] = serde_json::to_value(leaves).unwrap_or(Value::Null);
    }
    input
}

/// A 256-bit random per-attempt token (the §6.4 fallback credential).
fn mint_token() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Inject the per-attempt credential and launch identity into the rendered
/// manifest's task container. TODO(Phase 2b): the projected ServiceAccount
/// token replaces the env credential; an env token is readable by anyone with
/// `pods get` in the namespace, which is why it must not outlive the spike.
fn inject_attempt_env(
    job: &mut er_k8s::k8s_openapi::api::batch::v1::Job,
    job_name: &str,
    token: &str,
    service_account: &str,
) {
    use er_k8s::k8s_openapi::api::core::v1::EnvVar;
    if let Some(spec) = job.spec.as_mut()
        && let Some(pod) = spec.template.spec.as_mut()
        && let Some(container) = pod.containers.first_mut()
    {
        pod.service_account_name = Some(service_account.to_string());
        pod.automount_service_account_token = Some(false);
        let env = container.env.get_or_insert_with(Vec::new);
        env.push(EnvVar {
            name: "ER_ATTEMPT_TOKEN".into(),
            value: Some(token.to_string()),
            value_from: None,
        });
        env.push(EnvVar {
            name: "ER_K8S_JOB_NAME".into(),
            value: Some(job_name.to_string()),
            value_from: None,
        });
    }
}

impl JobKind {
    /// The kind behind a task row, read from its job row (the row itself does
    /// not carry the kind).
    async fn parse_row_kind(client: &Client, row: &TaskInstance) -> Option<JobKind> {
        let job = pg::get_job(client, &row.org_id, &row.job_id).await.ok()??;
        JobKind::parse(&job.kind)
    }
}

/// Parse only an operator/chart-provided enrollment list. A malformed list
/// disables the executor; it must not silently become an unscoped identity.
fn parse_runner_orgs(raw: Option<&str>) -> Result<BTreeSet<String>, &'static str> {
    let Some(raw) = raw else {
        return Ok(BTreeSet::new());
    };
    let orgs: BTreeSet<String> = serde_json::from_str(raw)
        .map_err(|_| "ER_TENANT_RUNNER_ORGS must be a JSON array of org ids")?;
    if orgs.iter().any(|org| org.trim().is_empty()) {
        return Err("ER_TENANT_RUNNER_ORGS contains an empty org id");
    }
    Ok(orgs)
}

/// The naming recipe is shared with Helm and Terraform: twenty hex digits of
/// SHA-256 over the exact org id. Hashing keeps arbitrary ids out of YAML and
/// Kubernetes names; enrollment and the IAM trust, not the name, grant access.
fn runner_service_account(org_id: &str, enrolled: &BTreeSet<String>) -> String {
    if enrolled.contains(org_id) {
        format!("er-runner-{}", &pg::hash_secret(org_id)[..20])
    } else {
        "er-runner".to_string()
    }
}

#[cfg(test)]
mod runner_identity_tests {
    use super::*;

    #[test]
    fn tenant_identity_is_enrolled_and_never_inherited() {
        let enrolled = parse_runner_orgs(Some(r#"["tenant-a","tenant-b"]"#)).unwrap();
        let a = runner_service_account("tenant-a", &enrolled);
        let b = runner_service_account("tenant-b", &enrolled);
        assert_ne!(a, b);
        assert_eq!(a, "er-runner-80a707af7dc77ee1228f");
        assert_eq!(runner_service_account("tenant-c", &enrolled), "er-runner");
        assert_eq!(
            runner_service_account("tenant-a", &BTreeSet::new()),
            "er-runner"
        );
    }

    #[test]
    fn malformed_enrollment_is_refused() {
        for bad in ["", "null", "{}", "[42]", r#"[""]"#, r#"["  "]"#] {
            assert!(parse_runner_orgs(Some(bad)).is_err(), "{bad}");
        }
        assert!(parse_runner_orgs(None).unwrap().is_empty());
        assert!(parse_runner_orgs(Some("[]")).unwrap().is_empty());
    }

    #[test]
    fn launch_identity_overrides_template_and_keeps_kubernetes_credentials_off() {
        use er_k8s::k8s_openapi::api::{
            batch::v1::JobSpec,
            core::v1::{Container, PodSpec},
        };
        let mut job = er_k8s::k8s_openapi::api::batch::v1::Job {
            spec: Some(JobSpec::default()),
            ..Default::default()
        };
        let pod = job
            .spec
            .as_mut()
            .unwrap()
            .template
            .spec
            .insert(PodSpec::default());
        pod.service_account_name = Some("er-api".into());
        pod.automount_service_account_token = Some(true);
        pod.containers.push(Container::default());
        let enrolled = parse_runner_orgs(Some(r#"["tenant-a","tenant-b"]"#)).unwrap();
        let chosen = runner_service_account("tenant-a", &enrolled);
        inject_attempt_env(&mut job, "attempt-1", "test-token", &chosen);
        let pod = job.spec.unwrap().template.spec.unwrap();
        assert_eq!(pod.service_account_name.as_deref(), Some(chosen.as_str()));
        assert_eq!(pod.automount_service_account_token, Some(false));
    }
}
