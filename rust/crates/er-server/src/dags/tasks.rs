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
use super::edges::{Assembled, Labeled, Landed, Resolved, Scored, Standardized};
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

declare_task!(
    /// The reconcile half of `init` for kinds with no run row (§3.5); for
    /// `lake_migrate` the reconcile IS the work.
    ReconcileSchema, "reconcile_schema", (), Plan<SchemaReconciled>, writes: true
);

declare_task!(
    /// `flush_inlined_data` + `merge_adjacent_files` + `expire_snapshots`.
    Maintain, "maintain", (Plan<SchemaReconciled>,), Plan<MaintenanceReport>, writes: true
);

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
