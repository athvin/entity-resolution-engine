//! The plan documents (§3.2): small typed JSON stored on the producer's row
//! (`task_instances.output`), consumed positionally downstream.

use serde::{Deserialize, Serialize};

use er_dag::PlanDoc;

/// `init`'s output, the one document every downstream task consumes (§3.5):
/// decisions are made once, in `init`, and travel — a clear hours later cannot
/// flip the mode under a half-finished run (§9.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunPlan {
    pub mode: String,
    pub rebuild_reason: Option<String>,
    pub model_version: Option<String>,
    pub tf_snapshot_id: Option<String>,
    pub lexicon_hash: Option<String>,
    /// Lake snapshot at plan time; the §9.5 head check's baseline.
    pub snapshot_start: Option<i64>,
    /// The run's logical clock.
    pub run_at: String,
    /// The corpus size `init` measured; feeds the next run's `FromRun` class
    /// resolution (§3.7).
    pub corpus_records: Option<i64>,
}
impl PlanDoc for RunPlan {
    const STABLE_NAME: &'static str = "run_plan";
}

/// `drain_actions`' snapshot of the pending staged-action ids (§3.6); a re-run
/// after a crash re-snapshots, which is correct because nothing downstream has
/// run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrainedActions {
    pub action_ids: Vec<String>,
}
impl PlanDoc for DrainedActions {
    const STABLE_NAME: &'static str = "drained_actions";
}

/// `reconcile_schema`'s report, mirrored into `tenant_schema_states`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaReconciled {
    pub status: String,
    pub catalog_hash: String,
    /// The reconciler's human detail, recorded into `tenant_schema_states` by
    /// the finish handler (the pod holds no control-plane credential, §6.4).
    pub detail: String,
}
impl PlanDoc for SchemaReconciled {
    const STABLE_NAME: &'static str = "schema_reconciled";
}

/// `train`'s output: the per-run artifact paths the registry row points at
/// (§9.1 — the shared-CSV append is deleted with the legacy arm).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainedModel {
    pub model_version: String,
    pub tf_snapshot_id: String,
    /// Where the pod wrote the settings document, on the shared filesystem
    /// both halves mount — the plan carries the PATH rather than the document
    /// because a registry row is the control task's write, not the pod's.
    pub model_path: String,
    pub tf_path: String,
    /// `person | account`, from the config the pod read. The registry row
    /// needs it and the control task has no config in scope.
    #[serde(default)]
    pub entity_type: String,
    /// Provenance the registry row records: which config produced this
    /// version, and how many labels the `m` estimate actually USED (zero means
    /// the EM estimate stands).
    #[serde(default)]
    pub config_hash: String,
    #[serde(default)]
    pub labels_used: usize,
}
impl PlanDoc for TrainedModel {
    const STABLE_NAME: &'static str = "trained_model";
}

/// `register_model`'s receipt: the draft registry row's label.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRegistered {
    pub label: String,
}
impl PlanDoc for ModelRegistered {
    const STABLE_NAME: &'static str = "model_registered";
}

/// `maintain`'s before/after stats document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenanceReport {
    pub report: serde_json::Value,
}
impl PlanDoc for MaintenanceReport {
    const STABLE_NAME: &'static str = "maintenance_report";
}

/// `create_catalog`'s receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogCreated {
    pub tenant: String,
}
impl PlanDoc for CatalogCreated {
    const STABLE_NAME: &'static str = "catalog_created";
}

/// `lake_init`'s receipt: the reconciled lake's catalog hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LakeInitialized {
    pub catalog_hash: String,
}
impl PlanDoc for LakeInitialized {
    const STABLE_NAME: &'static str = "lake_initialized";
}

/// `activate`'s receipt (`set_org_state(provisioning → active)`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgActivated {
    pub activated: bool,
}
impl PlanDoc for OrgActivated {
    const STABLE_NAME: &'static str = "org_activated";
}

/// `settle_actions`' receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettledActions {
    pub applied: i64,
}
impl PlanDoc for SettledActions {
    const STABLE_NAME: &'static str = "settled_actions";
}

/// `apply_staged`'s receipt: what the lake took, and what Postgres must still
/// record for it.
///
/// Carries a payload because the derived pair labels and review resolutions
/// are POSTGRES writes and `apply_staged` is a pod — it cannot make them
/// (§6.4). The ledger is the only channel between the two halves, so they
/// ride here to `settle_actions`, which is the control task that can.
///
/// `applied` is the ids that landed, in order. Applying stops at the first
/// failing action, so this is a prefix of the drained set: the rest stay
/// pending for a later pass rather than being lost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagedApplied {
    pub applied: Vec<String>,
    /// The pair labels the actions implied, with the actor who decided each:
    /// a label must name the PERSON, not the role (the D7 lesson).
    #[serde(default)]
    pub labels: Vec<DerivedPairLabel>,
    /// `(review_id, resolution)` for the actions that were review resolutions.
    #[serde(default)]
    pub resolutions: Vec<(String, String)>,
    pub snapshot_end: Option<i64>,
}
impl PlanDoc for StagedApplied {
    const STABLE_NAME: &'static str = "staged_applied";
}

/// One derived pair label as it crosses the ledger: the engine's
/// `er_cli::DerivedLabel` plus the actor, flattened so the plan document is
/// self-describing JSON rather than a tuple nobody can read in a row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivedPairLabel {
    pub rec_a_key: String,
    pub rec_b_key: String,
    /// `match` or `no_match`. A dismissal yields no label at all — "not worth
    /// my time" is not a verdict about whether the pair is the same person.
    pub verdict: String,
    pub decided_by: Option<String>,
}

/// `stage_approved`'s receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagedApproved {
    pub staged: i64,
    pub enqueued: bool,
}
impl PlanDoc for StagedApproved {
    const STABLE_NAME: &'static str = "staged_approved";
}

/// `finish`'s record of the settle verdict it composed (`finish` consumes no
/// lake edge; its work order carries the leaf verdicts — §4.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinishReport {
    pub settled: String,
}
impl PlanDoc for FinishReport {
    const STABLE_NAME: &'static str = "finish_report";
}
