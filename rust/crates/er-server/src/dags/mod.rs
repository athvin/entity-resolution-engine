//! The compiled DAGs: one per job kind, in the leaf binary (dagr's own rule —
//! the registration lives where the dispatcher, the API and the pod all are).
//! `dag_for` is an exhaustive match, so a kind without a DAG does not compile;
//! [`verify_all`] runs every assembly at boot, so a graph problem is a refusal
//! to start, not a log line (batch-orchestration-design §3.2/§3.3).
//!
//! Phase 1 ships the model and the compiled graphs with **no executor**: the
//! task bodies are declared (their shapes are what the compile-time wiring
//! checks) and refuse to run until `er-server exec-task` lands in Phase 2.

pub mod control;
pub mod edges;
pub mod plans;
pub mod tasks;

use er_dag::{Dag, DagBuilder, TriggerRule};

use control::{Activate, CreateCatalog, DrainActions, RegisterModel, SettleActions, StageApproved};
use tasks::{
    ApplyStaged, Assemble, CorrectReconcile, CorrectionsInit, CorrectionsReconcile, Finish, Ingest,
    Init, LakeInit, Maintain, Match, ProfileCorpus, Reconcile, ReconcileSchema, Standardize, Train,
};

/// The lake session type every pod task body takes; fixed here so `er-dag`
/// never names the engine.
pub type Session = dedupe_core::Session;
/// The control-plane client every control task body takes.
pub type PgClient = tokio_postgres::Client;

type Builder = DagBuilder<Session, PgClient>;

/// The closed set of `jobs.kind` strings (§3.2). A `jobs.kind` that does not
/// parse is a terminal `config` failure at planning, named in `error_detail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    RunAllFull,
    RunAllIncremental,
    Correct,
    Train,
    LakeMaintain,
    Profile,
    LakeMigrate,
    Provision,
    DatasetProvision,
    ApplyCorrections,
    ApplyApproved,
}

impl JobKind {
    pub const ALL: [JobKind; 11] = [
        JobKind::RunAllFull,
        JobKind::RunAllIncremental,
        JobKind::Correct,
        JobKind::Train,
        JobKind::LakeMaintain,
        JobKind::Profile,
        JobKind::LakeMigrate,
        JobKind::Provision,
        JobKind::DatasetProvision,
        JobKind::ApplyCorrections,
        JobKind::ApplyApproved,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            JobKind::RunAllFull => "run_all_full",
            JobKind::RunAllIncremental => "run_all_incremental",
            JobKind::Correct => "correct",
            JobKind::Train => "train",
            JobKind::LakeMaintain => "lake_maintain",
            JobKind::Profile => "profile",
            JobKind::LakeMigrate => "lake_migrate",
            JobKind::Provision => "provision",
            JobKind::DatasetProvision => "dataset_provision",
            JobKind::ApplyCorrections => "apply_corrections",
            JobKind::ApplyApproved => "apply_approved",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        JobKind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// The DAG a kind runs. `run_all_full` and `run_all_incremental` share one
    /// graph — the mode is a run parameter `init` resolves through the drift
    /// guard, not a shape (§3.5).
    #[must_use]
    pub fn dag_name(self) -> &'static str {
        match self {
            JobKind::RunAllFull | JobKind::RunAllIncremental => "run_all",
            other => other.as_str(),
        }
    }
}

/// The registry. Exhaustive: a kind without a DAG does not compile.
///
/// # Panics
/// A DAG that fails assembly is a programming error; [`verify_all`] surfaces
/// it at boot and `tests/dag_assembly.rs` at PR time.
#[must_use]
pub fn dag_for(kind: JobKind) -> Dag {
    match kind {
        JobKind::RunAllFull | JobKind::RunAllIncremental => run_all(),
        JobKind::Correct => correct(),
        JobKind::Train => train(),
        JobKind::LakeMaintain => lake_maintain(),
        JobKind::Profile => profile(),
        JobKind::LakeMigrate => lake_migrate(),
        JobKind::Provision => provision(),
        JobKind::DatasetProvision => dataset_provision(),
        JobKind::ApplyCorrections => apply_corrections(),
        JobKind::ApplyApproved => apply_approved(),
    }
}

/// Assemble every kind's DAG; called at boot so a refusal is a refusal to
/// start.
pub fn verify_all() {
    for kind in JobKind::ALL {
        let _ = dag_for(kind);
    }
}

/// What one executed pod task hands the attempt channel (§6.3 step 7).
pub struct PodTaskRun {
    pub skipped: bool,
    pub outcome: &'static str,
    pub output: serde_json::Value,
    pub counters: Option<serde_json::Value>,
}

/// Rehydrate the i-th positional input pin from the work order.
fn pin<T: er_dag::DurableOutput>(
    ctx: &er_dag::TaskCtx<'_, Session>,
    index: usize,
) -> Result<T, er_dag::TaskError> {
    let slot = ctx
        .order
        .input
        .get("pins")
        .and_then(|pins| pins.get(index))
        .ok_or_else(|| {
            er_dag::TaskError::Failed(er_errors::ErError::precondition(format!(
                "the work order carries no input pin {index} for {}",
                T::STABLE_NAME
            )))
        })?;
    T::rehydrate(slot).map_err(|e| {
        er_dag::TaskError::Failed(er_errors::ErError::precondition(format!(
            "input pin {index}: {}",
            crate::errfmt::chain(&e)
        )))
    })
}

fn completed<O: er_dag::DurableOutput>(
    outcome: er_dag::Outcome<O>,
) -> Result<PodTaskRun, er_dag::TaskError> {
    let (skipped, name, output, counters) = match outcome {
        er_dag::Outcome::Done(o, c) => (false, "done", o.serialize_reference(), Some(c.0)),
        er_dag::Outcome::NoOp(o) => (false, "no_op", o.serialize_reference(), None),
    };
    let output = output.map_err(|e| {
        er_dag::TaskError::Failed(er_errors::ErError::stage_failure(format!(
            "serializing the task's output reference: {}",
            crate::errfmt::chain(&e)
        )))
    })?;
    Ok(PodTaskRun {
        skipped,
        outcome: name,
        output,
        counters: counters.filter(|v| !v.is_null()),
    })
}

/// Run one pod task body by name — the verb's runtime dispatch (§6.3 step 5):
/// rehydrate the positional inputs, run exactly one try with no retry, backoff
/// or timeout of its own, serialize the durable output.
///
/// Phase 2 runs the maintenance kinds; every other body still refuses, and the
/// refusal is a `precondition` failure on the row rather than a silent skip.
///
/// # Errors
/// The task's own classified failure, or a malformed input reference.
/// Run one CONTROL task body (§3.6): the Postgres half of a kind, executed
/// inside the dispatcher with the control plane's own credentials.
///
/// Mirrors [`run_pod_task`] and is deliberately separate: a control body is
/// structurally unable to open a lake (`tests/control_tasks_no_engine.rs`
/// enforces that by source scan), and a pod body is structurally unable to
/// reach Postgres. The dispatch table is what decides which half a task is.
///
/// # Errors
/// The body's own failure, or a precondition refusal for a task this build
/// does not run.
pub async fn run_control_task(
    kind: JobKind,
    task_id: &str,
    ctx: &mut er_dag::ControlCtx<'_, PgClient>,
) -> Result<PodTaskRun, er_dag::TaskError> {
    use er_dag::ControlTask as _;
    match (kind, task_id) {
        (JobKind::Train, "register_model") => {
            let trained = control_pin::<er_dag::Plan<plans::TrainedModel>>(ctx, 0)?;
            completed(control::RegisterModel.run(ctx, (trained,)).await?)
        }
        (kind, task) => Err(er_dag::TaskError::Failed(er_errors::ErError::precondition(
            format!(
                "the dispatcher does not yet run control task '{task}' of kind '{}' — the \
                 remaining control bodies arrive with the rest of Phase 3",
                kind.as_str()
            ),
        ))),
    }
}

/// Rehydrate the i-th positional input pin of a CONTROL task.
///
/// The same `input.pins` array a pod task reads — a control body's upstream is
/// a durable output like any other, and `register_model` consuming `train`'s
/// `TrainedModel` plan is exactly that.
fn control_pin<T: er_dag::DurableOutput>(
    ctx: &er_dag::ControlCtx<'_, PgClient>,
    index: usize,
) -> Result<T, er_dag::TaskError> {
    let pins = ctx.order.input.get("pins").and_then(|p| p.as_array());
    let Some(v) = pins.and_then(|p| p.get(index)) else {
        return Err(er_dag::TaskError::Failed(er_errors::ErError::precondition(
            format!("control input pin {index} is missing from the work order"),
        )));
    };
    T::rehydrate(v).map_err(|e| {
        er_dag::TaskError::Failed(er_errors::ErError::precondition(format!(
            "control input pin {index} is not the declared shape: {e}"
        )))
    })
}

pub fn run_pod_task(
    kind: JobKind,
    task_id: &str,
    ctx: &mut er_dag::TaskCtx<'_, Session>,
) -> Result<PodTaskRun, er_dag::TaskError> {
    use er_dag::Task as _;
    match (kind, task_id) {
        (JobKind::LakeMaintain | JobKind::LakeMigrate | JobKind::Train, "reconcile_schema") => {
            completed(tasks::ReconcileSchema.run(ctx, ())?)
        }
        (JobKind::LakeMaintain, "maintain") => {
            let schema = pin::<er_dag::Plan<plans::SchemaReconciled>>(ctx, 0)?;
            completed(tasks::Maintain.run(ctx, (schema,))?)
        }
        (JobKind::Train, "train") => {
            let schema = pin::<er_dag::Plan<plans::SchemaReconciled>>(ctx, 0)?;
            completed(tasks::Train.run(ctx, (schema,))?)
        }
        (JobKind::Profile, "profile") => {
            let schema = pin::<er_dag::Plan<plans::SchemaReconciled>>(ctx, 0)?;
            completed(tasks::ProfileCorpus.run(ctx, (schema,))?)
        }
        // The pipeline stages (§11 Phase 3). Each rehydrates its upstream
        // references POSITIONALLY, in the order the DAG wired them — which is
        // what the typed `Input` tuples make checkable at compile time.
        (JobKind::RunAllFull | JobKind::RunAllIncremental, "init") => {
            completed(tasks::Init.run(ctx, ())?)
        }
        (JobKind::RunAllFull | JobKind::RunAllIncremental, "ingest") => {
            let plan = pin::<er_dag::Plan<plans::RunPlan>>(ctx, 0)?;
            completed(tasks::Ingest.run(ctx, (plan,))?)
        }
        (JobKind::RunAllFull | JobKind::RunAllIncremental, "standardize") => {
            let plan = pin::<er_dag::Plan<plans::RunPlan>>(ctx, 0)?;
            let landed = pin::<er_dag::LakeRef<edges::Landed>>(ctx, 1)?;
            completed(tasks::Standardize.run(ctx, (plan, landed))?)
        }
        (JobKind::RunAllFull | JobKind::RunAllIncremental, "match") => {
            let plan = pin::<er_dag::Plan<plans::RunPlan>>(ctx, 0)?;
            let std = pin::<er_dag::LakeRef<edges::Standardized>>(ctx, 1)?;
            completed(tasks::Match.run(ctx, (plan, std))?)
        }
        (JobKind::RunAllFull | JobKind::RunAllIncremental, "reconcile") => {
            let plan = pin::<er_dag::Plan<plans::RunPlan>>(ctx, 0)?;
            let scored = pin::<er_dag::LakeRef<edges::Scored>>(ctx, 1)?;
            completed(tasks::Reconcile.run(ctx, (plan, scored))?)
        }
        // `assemble` is the SAME stage for a run and a correction pass: both
        // hand it a `Resolved` partition and ask for golden records, reviews,
        // metrics and quality over it.
        (JobKind::RunAllFull | JobKind::RunAllIncremental | JobKind::Correct, "assemble") => {
            let plan = pin::<er_dag::Plan<plans::RunPlan>>(ctx, 0)?;
            let ents = pin::<er_dag::LakeRef<edges::Resolved>>(ctx, 1)?;
            completed(tasks::Assemble.run(ctx, (plan, ents))?)
        }
        // The teardown takes NO input: it fires on whatever its upstreams did,
        // including an `init` that failed before publishing a plan (§4.3).
        (JobKind::RunAllFull | JobKind::RunAllIncremental | JobKind::Correct, "finish") => {
            completed(tasks::Finish.run(ctx, ())?)
        }
        // `correct` shares `init` (which branches on the kind) with `run_all`;
        // only its reconcile differs.
        (JobKind::Correct, "init") => completed(tasks::Init.run(ctx, ())?),
        (JobKind::Correct, "reconcile") => {
            let plan = pin::<er_dag::Plan<plans::RunPlan>>(ctx, 0)?;
            completed(tasks::CorrectReconcile.run(ctx, (plan,))?)
        }

        (kind, task) => Err(er_dag::TaskError::Failed(er_errors::ErError::precondition(
            format!(
                "the executor does not yet run task '{task}' of kind '{}' — its body arrives \
                 with the remaining Phase 3 kinds of batch-orchestration-design.md",
                kind.as_str()
            ),
        ))),
    }
}

fn run_all() -> Dag {
    let mut d = Builder::new(JobKind::RunAllFull.dag_name());
    let plan = d.source::<Init>();
    let raw = d.then::<Ingest>((plan,));
    let std = d.then::<Standardize>((plan, raw));
    let scored = d.then::<Match>((plan, std));
    let ents = d.then::<Reconcile>((plan, scored));
    let done = d.then::<Assemble>((plan, ents));
    d.epilogue::<Finish>(&[done.id()], TriggerRule::AllTerminal);
    d.build().expect("the run_all DAG assembles")
}

fn correct() -> Dag {
    let mut d = Builder::new(JobKind::Correct.dag_name());
    let plan = d.source::<Init>();
    let ents = d.then::<CorrectReconcile>((plan,));
    let done = d.then::<Assemble>((plan, ents));
    d.epilogue::<Finish>(&[done.id()], TriggerRule::AllTerminal);
    d.build().expect("the correct DAG assembles")
}

fn train() -> Dag {
    let mut d = Builder::new(JobKind::Train.dag_name());
    let schema = d.source::<ReconcileSchema>();
    let model = d.then::<Train>((schema,));
    let _ = d.control::<RegisterModel>((model,));
    d.build().expect("the train DAG assembles")
}

fn lake_maintain() -> Dag {
    let mut d = Builder::new(JobKind::LakeMaintain.dag_name());
    let schema = d.source::<ReconcileSchema>();
    let _ = d.then::<Maintain>((schema,));
    d.build().expect("the lake_maintain DAG assembles")
}

/// Profile one corpus stage. A single task, like `lake_maintain`: the profile is
/// read-mostly and its only write is its own three relations, so there is
/// nothing to fan out and no downstream task to hand an edge to.
fn profile() -> Dag {
    let mut d = Builder::new(JobKind::Profile.dag_name());
    let schema = d.source::<ReconcileSchema>();
    let _ = d.then::<ProfileCorpus>((schema,));
    d.build().expect("the profile DAG assembles")
}

fn lake_migrate() -> Dag {
    let mut d = Builder::new(JobKind::LakeMigrate.dag_name());
    // The reconcile IS the work (§3.5).
    let _ = d.source::<ReconcileSchema>();
    d.build().expect("the lake_migrate DAG assembles")
}

fn provision() -> Dag {
    let mut d = Builder::new(JobKind::Provision.dag_name());
    let catalog = d.control::<CreateCatalog>(());
    let lake = d.then::<LakeInit>((catalog,));
    let _ = d.control::<Activate>((lake,));
    d.build().expect("the provision DAG assembles")
}

fn dataset_provision() -> Dag {
    // The provision shape, re-ledgered per dataset. CreateCatalog is
    // idempotent on the org's existing metadata database; the dataset's own
    // catalog (a METADATA_SCHEMA inside it, or a local file) is created by the
    // lake's first attach inside LakeInit.
    let mut d = Builder::new(JobKind::DatasetProvision.dag_name());
    let catalog = d.control::<CreateCatalog>(());
    let lake = d.then::<LakeInit>((catalog,));
    let _ = d.control::<Activate>((lake,));
    d.build().expect("the dataset_provision DAG assembles")
}

fn apply_corrections() -> Dag {
    let mut d = Builder::new(JobKind::ApplyCorrections.dag_name());
    let drained = d.control::<DrainActions>(());
    let plan = d.then::<CorrectionsInit>((drained,));
    let labeled = d.then::<ApplyStaged>((plan,));
    let ents = d.then::<CorrectionsReconcile>((plan, labeled));
    let done = d.then::<Assemble>((plan, ents));
    let settled = d.control::<SettleActions>((drained, done));
    d.epilogue::<Finish>(&[settled.id()], TriggerRule::AllTerminal);
    d.build().expect("the apply_corrections DAG assembles")
}

fn apply_approved() -> Dag {
    let mut d = Builder::new(JobKind::ApplyApproved.dag_name());
    // Reads tags, stages actions, enqueues apply_corrections; never opens a
    // lake (§3.5).
    let _ = d.control::<StageApproved>(());
    d.build().expect("the apply_approved DAG assembles")
}
