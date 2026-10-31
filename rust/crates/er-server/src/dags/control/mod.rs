//! The control task types (§3.6): the Postgres halves of kinds that already
//! have them, visible, idempotent and retryable under the same ledger
//! discipline as a pod task — and structurally unable to run a stage.
//!
//! **This module must never name `er_cli`, `dedupe_core` or `open_lake`**:
//! `tests/control_tasks_no_engine.rs` scans every file under `dags/control/`
//! for those needles, the same structural-claim-by-source-scan
//! `tests/egress_guard.rs` uses. A control body runs inside the dispatcher
//! with the control plane's own credentials; a body that needs a lake open is
//! a pod task by rule (§3.5 — `provision` was split for exactly this).
//!
//! Phase 1 declares, Phase 2 executes: bodies refuse until the dispatcher's
//! control-claim loop lands.

use er_dag::{ControlCtx, ControlTask, Outcome, Plan, TaskError};

use super::PgClient;
use super::plans::{
    CatalogCreated, DrainedActions, LakeInitialized, ModelRegistered, OrgActivated, SettledActions,
    StagedApproved, TrainedModel,
};

fn not_yet_runnable(task: &str) -> TaskError {
    TaskError::Failed(er_errors::ErError::precondition(format!(
        "control task body '{task}' is declared but not yet runnable: the dispatcher's control \
         loop is Phase 2 of batch-orchestration-design.md"
    )))
}

macro_rules! declare_control_task {
    ($(#[$doc:meta])* $ty:ident, $name:literal, $input:ty, $output:ty) => {
        $(#[$doc])*
        #[derive(Default)]
        pub struct $ty;

        impl ControlTask<PgClient> for $ty {
            const NAME: &'static str = $name;
            const VERSION: u32 = 1;
            type Input = $input;
            type Output = $output;

            fn run(
                &mut self,
                _ctx: &mut ControlCtx<'_, PgClient>,
                _input: Self::Input,
            ) -> Result<Outcome<Self::Output>, TaskError> {
                Err(not_yet_runnable($name))
            }
        }
    };
}

declare_control_task!(
    /// Creates the tenant database on the maintenance DSN (or the local lake
    /// dir) — explicitly non-transactional, with the `pg_database` probe and
    /// `create_dir_all` as its idempotency keys (§3.6).
    CreateCatalog, "create_catalog", (), Plan<CatalogCreated>
);

declare_control_task!(
    /// `set_org_state(provisioning → active)`; a false return with the state
    /// already `active` is success (§3.6).
    Activate, "activate", (Plan<LakeInitialized>,), Plan<OrgActivated>
);

declare_control_task!(
    /// Creates the draft `trained-{run_id}` registry row pointing at the
    /// per-run TF path; probes the registry for the label first (§3.6, §9.1).
    RegisterModel, "register_model", (Plan<TrainedModel>,), Plan<ModelRegistered>
);

declare_control_task!(
    /// Snapshots the pending staged-action ids as `Plan<DrainedActions>`,
    /// `init`'s upstream (§3.5).
    DrainActions, "drain_actions", (), Plan<DrainedActions>
);

declare_control_task!(
    /// Marks applied, writes reviewer labels, emits `review.resolved` — all
    /// Postgres rows, in one transaction with its finish (§3.6).
    SettleActions, "settle_actions", (Plan<DrainedActions>, er_dag::LakeRef<super::edges::Assembled>), Plan<SettledActions>
);

declare_control_task!(
    /// Reads tags, stages actions, enqueues `apply_corrections`; never opens a
    /// lake (§3.5).
    StageApproved, "stage_approved", (), Plan<StagedApproved>
);
