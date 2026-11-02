//! The pod task types: the typed shapes `dag_for`'s wiring is checked against.
//!
//! **Phase 1 declares, Phase 2 executes.** Every body here refuses with a
//! `precondition` failure naming the missing executor; nothing routes a job to
//! these bodies until `er-server exec-task` lands. The declared `Input`/
//! `Output` types are load-bearing now — they are what makes a mis-wired edge
//! a compile error — and the bodies become real by calling the `er_cli` stage
//! functions behind the same signatures.

use er_dag::{LakeRef, NodePolicy, Outcome, Plan, Task, TaskCtx, TaskError};

use super::Session;
use super::edges::{Assembled, Labeled, Landed, Profiled, Resolved, Scored, Standardized};
use super::plans::{
    DrainedActions, FinishReport, LakeInitialized, MaintenanceReport, RunPlan, SchemaReconciled,
    TrainedModel,
};

fn not_yet_runnable(task: &str) -> TaskError {
    TaskError::Failed(er_errors::ErError::precondition(format!(
        "task body '{task}' is declared but not yet runnable: the Kubernetes executor is Phase 2 \
         of batch-orchestration-design.md"
    )))
}

macro_rules! declare_task {
    ($(#[$doc:meta])* $ty:ident, $name:literal, $input:ty, $output:ty, writes: $writes:literal $(, policy: $policy:expr)?) => {
        $(#[$doc])*
        #[derive(Default)]
        pub struct $ty;

        impl Task<Session> for $ty {
            const NAME: &'static str = $name;
            const VERSION: u32 = 1;
            type Input = $input;
            type Output = $output;
            const WRITES_LAKE: bool = $writes;

            $(fn policy() -> NodePolicy { $policy })?

            fn run(
                &mut self,
                _ctx: &mut TaskCtx<'_, Session>,
                _input: Self::Input,
            ) -> Result<Outcome<Self::Output>, TaskError> {
                Err(not_yet_runnable($name))
            }
        }
    };
}

declare_task!(
    /// Opens the lake, reconciles the schema once, makes the drift-guard mode
    /// decision once, deletes-then-inserts the `runs` row, and emits the
    /// `RunPlan` every downstream task consumes (§3.5).
    Init, "init", (), Plan<RunPlan>, writes: true
);

declare_task!(
    /// Pre-images the whole run (`raw_record/delete/by_batch` +
    /// `ingest_batch/delete/by_run`), then lands every delivery; fan-out over
    /// N deliveries is the task looping internally (§3.1, §9.1).
    Ingest, "ingest", (Plan<RunPlan>,), LakeRef<Landed>, writes: true
);

declare_task!(
    /// Drop-and-rebuild of `int_std_records`.
    Standardize, "standardize", (Plan<RunPlan>, LakeRef<Landed>), LakeRef<Standardized>, writes: true
);

declare_task!(
    /// The memory and thread consumer; re-asserts the plan's fingerprint
    /// inside its own transaction and fails `precondition` on a straddled
    /// config publish (§9.5). `screen_deliveries` is its tail.
    Match, "match", (Plan<RunPlan>, LakeRef<Standardized>), LakeRef<Scored>, writes: true,
    policy: NodePolicy { escalate_on_oom: true, ..NodePolicy::default() }
);

declare_task!(
    /// Membership/entities MERGE plus the run's `entity_events`, pre-imaged by
    /// `entity_event/delete/by_run` (§9.1).
    Reconcile, "reconcile", (Plan<RunPlan>, LakeRef<Scored>), LakeRef<Resolved>, writes: true,
    policy: NodePolicy { escalate_on_oom: true, ..NodePolicy::default() }
);

declare_task!(
    /// golden + reviews + metrics + quality (`RUN_STAGES`' `assemble`).
    Assemble, "assemble", (Plan<RunPlan>, LakeRef<Resolved>), LakeRef<Assembled>, writes: true
);

declare_task!(
    /// The teardown epilogue: composes the leaf verdicts the readiness pass
    /// hands it and writes `runs.status` (`run/update/fail` for a non-success)
    /// — which is why it runs under `canceling` too (§4.3, §5.6).
    Finish, "finish", (), Plan<FinishReport>, writes: true,
    policy: NodePolicy { teardown: true, ..NodePolicy::default() }
);

declare_task!(
    /// `correct`'s reconcile: re-clusters from the lake's own scores, pinned
    /// to the model identity `init` read from `run/read/latest_live` (§3.5).
    CorrectReconcile, "reconcile", (Plan<RunPlan>,), LakeRef<Resolved>, writes: true,
    policy: NodePolicy { escalate_on_oom: true, ..NodePolicy::default() }
);

declare_task!(
    /// `apply_corrections`' init: consumes the drained-action snapshot and
    /// emits the pass's `RunPlan` (§3.5).
    CorrectionsInit, "init", (Plan<DrainedActions>,), Plan<RunPlan>, writes: true
);

declare_task!(
    /// Applies the drained staged actions to the lake (assertions, review
    /// resolutions): pod-side because the payload needs a lake open (§3.5).
    ApplyStaged, "apply_staged", (Plan<RunPlan>,), LakeRef<Labeled>, writes: true
);

declare_task!(
    /// `apply_corrections`' reconcile: consumes the freshly-landed labels.
    CorrectionsReconcile, "reconcile", (Plan<RunPlan>, LakeRef<Labeled>), LakeRef<Resolved>, writes: true,
    policy: NodePolicy { escalate_on_oom: true, ..NodePolicy::default() }
);

/// The reconcile half of `init` for kinds with no run row (§3.5); for
/// `lake_migrate` the reconcile IS the work. REAL since Phase 2: the first
/// body the executor runs.
#[derive(Default)]
pub struct ReconcileSchema;

impl Task<Session> for ReconcileSchema {
    const NAME: &'static str = "reconcile_schema";
    const VERSION: u32 = 1;
    type Input = ();
    type Output = Plan<SchemaReconciled>;
    const WRITES_LAKE: bool = true;

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        (): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        // `reconcile_lake_env` opens its own session from the tenant env — the
        // ctx session stays the probe/ledger handle.
        let outcome = crate::schema::reconcile_lake_env(&ctx.order.lake_env);
        let (status, hash, detail) = crate::schema::tenant_state(&outcome);
        match outcome {
            Ok(_) => Ok(Outcome::Done(
                Plan(SchemaReconciled {
                    status: status.to_string(),
                    catalog_hash: hash,
                    detail,
                }),
                er_dag::Counters::default(),
            )),
            Err(message) => Err(engine_error(&message)),
        }
    }
}

/// Profile one corpus stage and record it. REAL, like `Maintain`: the work is
/// one `er_cli` call with no fan-out, so there is nothing to wait for an
/// executor to supply.
///
/// The subject, tier and sampling come from the job's params rather than from
/// the lake env, because they are per-request rather than per-tenant — asking
/// for the golden records at tier 2 is a different question from asking for the
/// standardized corpus at tier 1, and the same tenant asks both.
#[derive(Default)]
pub struct ProfileCorpus;

impl Task<Session> for ProfileCorpus {
    const NAME: &'static str = "profile";
    const VERSION: u32 = 1;
    type Input = (Plan<SchemaReconciled>,);
    type Output = LakeRef<Profiled>;
    const WRITES_LAKE: bool = true;

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        _input: Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let request = crate::profile_params::ProfileRequest::from_params(&ctx.order.params)
            .map_err(|message| TaskError::Failed(er_errors::ErError::precondition(message)))?;
        let account = ctx
            .order
            .lake_env
            .get("ER_ENTITY_TYPE")
            .and_then(serde_json::Value::as_str)
            == Some("account");
        let captured_at = crate::dispatcher::fmt_epoch(crate::dispatcher::unix_now());
        match request.run(ctx.session, account, &captured_at) {
            Ok(report) => Ok(Outcome::Done(
                LakeRef::new(report.snapshot, report.column_count),
                er_dag::Counters(serde_json::json!({
                    "columns_profiled": report.column_count,
                    "rows_read": report.row_count,
                    "findings": report.findings.len(),
                })),
            )),
            Err(message) => Err(engine_error(&message)),
        }
    }
}

/// `flush_inlined_data` + `merge_adjacent_files` + `expire_snapshots`. REAL
/// since Phase 2.
#[derive(Default)]
pub struct Maintain;

impl Task<Session> for Maintain {
    const NAME: &'static str = "maintain";
    const VERSION: u32 = 1;
    type Input = (Plan<SchemaReconciled>,);
    type Output = Plan<MaintenanceReport>;
    const WRITES_LAKE: bool = true;

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        _input: Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let expire = ctx
            .order
            .lake_env
            .get("ER_EXPIRE_OLDER_THAN")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("7 days")
            .to_string();
        match er_cli::run_maintain(ctx.session, &expire) {
            Ok(report) => Ok(Outcome::Done(
                Plan(MaintenanceReport { report }),
                er_dag::Counters::default(),
            )),
            Err(e) => Err(engine_error(&format!("{e:#}"))),
        }
    }
}

/// Map an engine failure string onto the er-errors taxonomy the executor's
/// retry policy reads, through the same classifier the legacy arm uses.
fn engine_error(message: &str) -> TaskError {
    let class = crate::pg::FailureClass::classify(message);
    let err = match class {
        crate::pg::FailureClass::TransientIo => {
            er_errors::ErError::stage_failure(message.to_string())
                .with_class(er_errors::ErrorClass::TransientIo)
        }
        crate::pg::FailureClass::Precondition => {
            er_errors::ErError::precondition(message.to_string())
        }
        crate::pg::FailureClass::Config => er_errors::ErError::config(message.to_string()),
        crate::pg::FailureClass::Internal => er_errors::ErError::stage_failure(message.to_string()),
    };
    TaskError::Failed(err)
}

declare_task!(
    /// Trains and writes `models/<run_id>/{model.json,tf.csv}` on EFS; no lake
    /// write and no run row (§3.5, §9.1).
    Train, "train", (Plan<SchemaReconciled>,), Plan<TrainedModel>, writes: false
);

declare_task!(
    /// Creates and reconciles the new tenant's lake (today's
    /// `reconcile_lake_env` half of `provision_tenant`): anything opening a
    /// lake is a pod task (§3.5).
    LakeInit, "lake_init", (Plan<super::plans::CatalogCreated>,), Plan<LakeInitialized>, writes: true
);
