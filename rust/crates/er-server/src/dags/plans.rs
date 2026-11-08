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
    /// The drained actions themselves, which `apply_staged` applies.
    ///
    /// The payloads travel along this edge because a pod holds no
    /// control-plane credential (§6.4) and so cannot read
    /// `staged_steward_actions` for itself. They used to be described as
    /// riding in the order's `params`, and nothing ever put them there — so
    /// `apply_staged` applied nothing and the pass settled `succeeded`
    /// regardless (#760).
    ///
    /// `#[serde(default)]` so a row planned before this field existed still
    /// rehydrates; such a job is refused by `plan.sql`'s fingerprint guard
    /// anyway, and a rehydrate panic would be a worse way to say so.
    #[serde(default)]
    pub actions: Vec<DrainedAction>,
}

/// One staged steward action, as handed to the pod that applies it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrainedAction {
    pub id: String,
    pub action: serde_json::Value,
    /// Who staged it. Not optional: `staged_steward_actions.created_by` is
    /// `NOT NULL`, so the edge mirrors the row rather than widening it.
    pub created_by: String,
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

/// `lake_init`'s receipt: the reconciled lake's catalog hash, plus the two
/// fields `tenant_schema_states` needs.
///
/// It carries `status` and `detail` for the same reason `SchemaReconciled`
/// does — the finish handler writes that row, because the pod holds no
/// control-plane credential (§6.4). Without them a PROVISIONED tenant got no
/// row at all, so the deploy report and the drift check, which read the table
/// precisely to avoid opening a lake, could not see it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LakeInitialized {
    pub catalog_hash: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub detail: String,
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

/// `write_back_scan`'s output: the entities a correction pass changed, with the
/// master and members a subscriber needs, read from the CORRECTED corpus.
///
/// Scanned in a pod because resolving a record key to an entity and reading its
/// detail are lake reads, and queued by a control task because `webhook::emit`
/// is a Postgres write — the two halves §6.4 keeps apart, which is why this
/// document exists at all rather than one function doing both.
///
/// An empty list is the ordinary answer: a dismissal changes no master, and a
/// pass with no review resolutions writes nothing back.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WriteBackScanned {
    #[serde(default)]
    pub entities: Vec<WriteBackEntity>,
}
impl PlanDoc for WriteBackScanned {
    const STABLE_NAME: &'static str = "write_back_scanned";
}

/// One entity's write-back payload, as the webhook sends it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteBackEntity {
    pub entity_id: String,
    /// The golden record. `null` when the entity has none, which is carried
    /// rather than dropped so the event still names what changed.
    pub master: serde_json::Value,
    pub members: serde_json::Value,
}

/// `emit_write_back`'s receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteBackEmitted {
    pub queued: i64,
}
impl PlanDoc for WriteBackEmitted {
    const STABLE_NAME: &'static str = "write_back_emitted";
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

/// `extract`'s record of the delivery it staged from Salesforce: what landed in
/// the drop root, under which delivery id, and the new high-water mark.
///
/// It carries the identifiers `advance_sf_cursor` needs because the Postgres
/// writes — the run enqueue, the audit row, the cursor upsert — are the control
/// half's, and a pod holds no control-plane credential (§6.4). `watermark` is
/// the max updated-at value the pull actually saw, never a clock reading: a
/// server-side clock could sit ahead of a row still being committed in
/// Salesforce, and rows under a skipped mark never arrive again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractReport {
    pub connection_id: String,
    pub source: String,
    pub object: String,
    pub delivery_id: String,
    pub mode: String,
    pub rows: i64,
    pub bytes: i64,
    #[serde(default)]
    pub watermark: Option<String>,
}
impl PlanDoc for ExtractReport {
    const STABLE_NAME: &'static str = "extract_report";
}

/// `advance_sf_cursor`'s receipt: the run it enqueued (none for an empty pull)
/// and the mark it recorded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SfCursorAdvanced {
    #[serde(default)]
    pub enqueued_job: Option<String>,
    #[serde(default)]
    pub watermark: Option<String>,
}
impl PlanDoc for SfCursorAdvanced {
    const STABLE_NAME: &'static str = "sf_cursor_advanced";
}

/// One kind of entity event and how many of them a run produced.
///
/// Named fields rather than a `(String, i64)` tuple because this lands in a
/// task's durable output, which an operator reads in the task ledger: a
/// `{"event_type": "entity.created", "n": 12}` row says what it is, and
/// `["entity.created", 12]` does not.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityCount {
    pub event_type: String,
    pub n: i64,
}

/// `finish`'s record of the settle verdict it composed (`finish` consumes no
/// lake edge; its work order carries the leaf verdicts — §4.3), plus the
/// entity-event counts the webhook aggregate needs.
///
/// The counts ride in the OUTPUT because of where they can be read versus
/// where they can be sent. Counting is a lake query, and only a pod holds a
/// lake session (§3.5 draws that line at the lake and the control plane is on
/// the wrong side of it); emitting is a Postgres write, and only the control
/// plane holds that client. The legacy arm had both halves in one function and
/// its comment says exactly this — "Read HERE because this is the half that
/// holds the lake session; the half that can emit holds the Postgres client".
/// Split across a pod boundary, the task's durable output is the channel
/// between those halves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinishReport {
    pub settled: String,
    /// Empty for a run that reconciled nothing, which is the honest answer
    /// rather than a missing reading — including a `finish` that fired after a
    /// failed `init`, since it is a teardown and runs on `AllTerminal`.
    #[serde(default)]
    pub entity_counts: Vec<EntityCount>,
}
impl PlanDoc for FinishReport {
    const STABLE_NAME: &'static str = "finish_report";
}

#[cfg(test)]
mod finish_report_tests {
    use super::{EntityCount, FinishReport};
    use er_dag::DurableOutput as _;

    /// The counts must survive the POD BOUNDARY, and the boundary is JSON.
    ///
    /// `finish` runs in a pod and serializes its output; the control plane
    /// settles the job and digs the counts back out with a fixed key path
    /// (`output["doc"]["entity_counts"]`). Nothing type-checks that path — it
    /// is `Value::get` on strings — so a rename on either side would compile
    /// and then silently emit no aggregate, which is exactly the failure this
    /// whole change is fixing. This test is that path, written out.
    #[test]
    fn the_settle_path_recovers_what_finish_published() {
        let published = er_dag::Plan(FinishReport {
            settled: "closed".to_string(),
            entity_counts: vec![
                EntityCount {
                    event_type: "entity.created".to_string(),
                    n: 12,
                },
                EntityCount {
                    event_type: "entity.merged".to_string(),
                    n: 3,
                },
            ],
        })
        .serialize_reference()
        .expect("finish's output serializes");

        // Byte-for-byte the chain in `Executor::readiness_for`'s settle branch.
        let recovered: Vec<EntityCount> = published
            .get("doc")
            .and_then(|doc| doc.get("entity_counts"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .expect("the settle path finds the counts");

        let pairs: Vec<(String, i64)> =
            recovered.into_iter().map(|c| (c.event_type, c.n)).collect();
        assert_eq!(
            pairs,
            vec![
                ("entity.created".to_string(), 12),
                ("entity.merged".to_string(), 3)
            ]
        );
    }

    /// A run that reconciled nothing publishes an empty vector, and the settle
    /// path must read that as "nothing to announce" rather than as a missing
    /// reading. `finish` is a teardown on `AllTerminal`, so it also runs after
    /// a failed `init` that never opened a run row.
    #[test]
    fn an_empty_count_set_round_trips_as_empty() {
        let published = er_dag::Plan(FinishReport {
            settled: "no_run_row".to_string(),
            entity_counts: vec![],
        })
        .serialize_reference()
        .unwrap();
        let recovered: Vec<EntityCount> = published
            .get("doc")
            .and_then(|doc| doc.get("entity_counts"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .expect("an empty vector is still a reading");
        assert!(recovered.is_empty());
    }

    /// An output written before this field existed must still deserialize —
    /// `#[serde(default)]` is what makes a task row from an older binary
    /// readable, and a `finish_report` in the ledger outlives the release that
    /// wrote it.
    #[test]
    fn an_output_from_before_the_field_existed_still_reads() {
        let old = serde_json::json!({ "settled": "closed" });
        let report: FinishReport = serde_json::from_value(old).expect("the old shape deserializes");
        assert_eq!(report.settled, "closed");
        assert!(report.entity_counts.is_empty());
    }
}
