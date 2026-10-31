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
    pub model_path: String,
    pub tf_path: String,
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
