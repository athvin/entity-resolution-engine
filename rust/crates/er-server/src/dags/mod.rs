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
    ApplyStaged, Assemble, CorrectReconcile, CorrectionsInit, CorrectionsReconcile, Finish, Init,
    Ingest, LakeInit, Maintain, Match, Reconcile, ReconcileSchema, Standardize, Train,
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
    LakeMigrate,
    Provision,
    ApplyCorrections,
    ApplyApproved,
}

impl JobKind {
    pub const ALL: [JobKind; 9] = [
        JobKind::RunAllFull,
        JobKind::RunAllIncremental,
        JobKind::Correct,
        JobKind::Train,
        JobKind::LakeMaintain,
        JobKind::LakeMigrate,
        JobKind::Provision,
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
            JobKind::LakeMigrate => "lake_migrate",
            JobKind::Provision => "provision",
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
        JobKind::LakeMigrate => lake_migrate(),
        JobKind::Provision => provision(),
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
