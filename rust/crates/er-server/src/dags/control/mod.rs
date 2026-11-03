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
//! A control body is ASYNC (its work is Postgres) where a pod body is
//! synchronous (its work is DuckDB) — the two halves of a kind are shaped by
//! what they talk to.

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

            // A declared-but-unbuilt body has nothing to await, so the
            // future is spelled out: an `async fn` with no `.await` is a
            // clippy error, and the trait's method is async because the
            // REAL bodies are.
            fn run(
                &mut self,
                _ctx: &mut ControlCtx<'_, PgClient>,
                _input: Self::Input,
            ) -> impl ::std::future::Future<Output = Result<Outcome<Self::Output>, TaskError>> + Send
            {
                ::std::future::ready(Err(not_yet_runnable($name)))
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

/// Creates the draft `trained-{run_id}` registry row pointing at the per-run
/// TF path. REAL since Phase 3: the first control body the dispatcher runs.
///
/// Draft, never activated: a freshly trained version is a proposal, and
/// promoting it is a human act through the registry API.
///
/// Idempotent by probe (§9.1): the label is derived from the run id, so a
/// retry of this task finds its own row already there and succeeds rather
/// than creating a second draft of the same training pass.
#[derive(Default)]
pub struct RegisterModel;

impl ControlTask<PgClient> for RegisterModel {
    const NAME: &'static str = "register_model";
    const VERSION: u32 = 1;
    type Input = (Plan<TrainedModel>,);
    type Output = Plan<ModelRegistered>;

    async fn run(
        &mut self,
        ctx: &mut ControlCtx<'_, PgClient>,
        (trained,): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let trained = trained.0;
        // The pod wrote the settings document to the shared filesystem; this
        // half reads it and writes the row. Reading a path rather than taking
        // the document through the ledger keeps a multi-kilobyte blob out of
        // Postgres's task rows.
        let model_json = std::fs::read_to_string(&trained.model_path).map_err(|e| {
            TaskError::Failed(er_errors::ErError::precondition(format!(
                "the trained model is not readable at {}: {e}",
                trained.model_path
            )))
        })?;

        // The §9.1 probe: this label already registered means a prior try of
        // THIS task got there, which is success rather than a second draft of
        // the same training pass. The label is derived from the run id, so the
        // probe is exact — `list` rather than a by-label statement because the
        // registry of one org is small and this read happens once per train.
        let existing = er_entities::matching_model::read::list(ctx.client, &ctx.order.org_id)
            .await
            .map_err(|e| {
                TaskError::Failed(
                    er_errors::ErError::stage_failure(format!(
                        "probing the registry for '{}': {e}",
                        trained.model_version
                    ))
                    .with_class(er_errors::ErrorClass::TransientIo),
                )
            })?;
        if existing
            .iter()
            .any(|m| m.model_version_label == trained.model_version)
        {
            return Ok(Outcome::Done(
                Plan(ModelRegistered {
                    label: trained.model_version,
                }),
                er_dag::Counters(serde_json::json!({ "already_registered": true })),
            ));
        }

        let provenance = serde_json::json!({
            "source": "train",
            "job_id": ctx.order.job_id,
            "config_hash": trained.config_hash,
            "trained_from": trained.model_version,
            // How `m` was estimated, so a version's origin is legible from the
            // registry rather than inferred. Zero means the EM estimate
            // stands. Counts the labels actually USED, so it excludes
            // `no_match` and `origin = 'llm'` rows: a number that counted
            // suggestions the training then ignored would misdescribe every
            // draft it appears on.
            "m_from_labels": trained.labels_used,
        });
        crate::pg::create_model_version(
            ctx.client,
            &ctx.order.org_id,
            ctx.order.dataset_id.as_deref(),
            crate::pg::DEFAULT_MODEL_NAME,
            &trained.entity_type,
            &model_json,
            &trained.model_version,
            Some(&trained.tf_snapshot_id),
            &provenance,
            "trainer",
        )
        .await
        .map_err(|e| {
            // Retryable: a control task that lost its connection mid-write
            // re-probes on the next try and either finds its row or writes it.
            TaskError::Failed(
                er_errors::ErError::stage_failure(format!("registering the trained model: {e}"))
                    .with_class(er_errors::ErrorClass::TransientIo),
            )
        })?;
        Ok(Outcome::Done(
            Plan(ModelRegistered {
                label: trained.model_version,
            }),
            er_dag::Counters(serde_json::json!({ "labels_used": trained.labels_used })),
        ))
    }
}

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
