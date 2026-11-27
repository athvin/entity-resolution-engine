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
//!
//! Every body here is real. The `declare_control_task!` stub macro that stood
//! in for them is gone, which is the honest marker that the control half of
//! the design is built: there is no longer a control task the dispatcher
//! declares but cannot run.

use er_dag::{ControlCtx, ControlTask, Outcome, Plan, TaskError};

use super::PgClient;
use super::plans::{
    CatalogCreated, DrainedActions, ExtractReport, LakeInitialized, ModelRegistered, OrgActivated,
    SettledActions, SfCursorAdvanced, StagedApplied, StagedApproved, TrainedModel,
};

/// The actor an extract's delivery is attributed to — the mechanism, like
/// `s3-ingest` and `scheduler`: nobody authorised this particular import, and
/// attributing it to whoever registered the connection would make the audit
/// log read as though they had.
const SF_EXTRACT_ACTOR: &str = "sf-extract";

/// Publish what the `extract` pod staged: enqueue the incremental run that
/// ingests the delivery, write the audit row and the `import.received` event,
/// and only then advance the (dataset, source) watermark — the same
/// publish-then-advance order the S3 poller keeps, because a cursor advanced
/// over a failed publish skips those rows forever while the reverse at worst
/// re-extracts once.
///
/// The enqueue carries an idempotency key derived from the delivery id, so a
/// retry of this task (after, say, a cursor write that failed) re-finds the
/// job it already made rather than queueing the ingest twice.
#[derive(Default)]
pub struct AdvanceSfCursor;

impl ControlTask<PgClient> for AdvanceSfCursor {
    const NAME: &'static str = "advance_sf_cursor";
    const VERSION: u32 = 2;
    type Input = (Plan<ExtractReport>,);
    type Output = Plan<SfCursorAdvanced>;

    async fn run(
        &mut self,
        ctx: &mut ControlCtx<'_, PgClient>,
        (report,): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let report = report.0;
        if report.rows == 0 {
            // The pod found nothing new and published nothing — the ordinary
            // scheduled-incremental outcome. No run, no cursor movement.
            return Ok(Outcome::NoOp(Plan(SfCursorAdvanced {
                enqueued_job: None,
                watermark: None,
            })));
        }
        let Some(dataset_id) = ctx.order.dataset_id.clone() else {
            return Err(TaskError::Failed(er_errors::ErError::precondition(
                "an extract job must name a dataset".to_string(),
            )));
        };

        let pin = report.delivery.as_ref().ok_or_else(|| {
            TaskError::Failed(er_errors::ErError::precondition(
                "extract report has no immutable delivery; re-extract before advancing the cursor",
            ))
        })?;
        if pin.dataset_id != dataset_id
            || pin.source != report.source
            || pin.delivery_id != report.delivery_id
            || pin.version != 1
        {
            return Err(TaskError::Failed(er_errors::ErError::precondition(
                "extract report delivery scope does not match the job",
            )));
        }
        let idem = format!("extract:{}", report.delivery_id);
        let job = crate::pg::enqueue(
            ctx.client,
            &ctx.order.org_id,
            Some(dataset_id.as_str()),
            "run_all_incremental",
            0,
            Some(&idem),
            Some(&pin.params()),
            crate::pg::Provenance::actor(SF_EXTRACT_ACTOR),
        )
        .await
        .map_err(|e| {
            TaskError::Failed(
                er_errors::ErError::stage_failure(format!("enqueueing the ingest run: {e}"))
                    .with_class(er_errors::ErrorClass::TransientIo),
            )
        })?;

        // Provenance (#1296) is NOT best-effort: it is what may later let a
        // landed record acquire a Salesforce identity for writeback. Written
        // after the idempotent enqueue (the run id is the job id) and before
        // the cursor advances, so a failure retries this task, which re-finds
        // the same job and converges; a run whose provenance never lands is
        // simply ineligible — the closed failure.
        crate::pg::record_provenance(
            ctx.client,
            &crate::pg::DeliveryProvenance {
                org_id: &ctx.order.org_id,
                dataset_id: &dataset_id,
                run_id: &job.id,
                delivery_id: &report.delivery_id,
                source: &report.source,
                connection_id: &report.connection_id,
                object: &report.object,
                delivery_sha256: &pin.sha256,
                source_sha256: &pin.source_sha256,
            },
        )
        .await
        .map_err(|e| {
            let class = match e {
                crate::pg::ProvenanceError::Conflict(_) => er_errors::ErrorClass::Precondition,
                crate::pg::ProvenanceError::Db(_) => er_errors::ErrorClass::TransientIo,
            };
            TaskError::Failed(
                er_errors::ErError::stage_failure(format!(
                    "recording the delivery's provenance: {}",
                    crate::errfmt::chain(&e)
                ))
                .with_class(class),
            )
        })?;

        // Best-effort reporting, the finish_delivery discipline: the delivery
        // is committed and its run queued, so neither row may undo that.
        let _ = crate::pg::audit(
            ctx.client,
            SF_EXTRACT_ACTOR,
            Some(&ctx.order.org_id),
            Some(&dataset_id),
            "import.extract",
            &serde_json::json!({
                "source": report.source,
                "object": report.object,
                "connection_id": report.connection_id,
                "delivery_id": report.delivery_id,
                "rows": report.rows,
                "bytes": report.bytes,
                "job_id": job.id,
            }),
        )
        .await;
        let org_name = match crate::pg::get_org_by_id(ctx.client, &ctx.order.org_id).await {
            Ok(Some(org)) => org.name,
            _ => ctx.order.org_id.clone(),
        };
        if let Err(e) = crate::webhook::emit(
            ctx.client,
            &ctx.order.org_id,
            Some(dataset_id.as_str()),
            &org_name,
            "import.received",
            &serde_json::json!({
                "source": report.source,
                "delivery_id": report.delivery_id,
                "bytes": report.bytes,
                "job_id": job.id,
            }),
        )
        .await
        {
            tracing::warn!(er.source = %report.source, error = %e, "the import.received webhook was not queued");
        }

        // The advance comes LAST. A failure here retries the task; the enqueue
        // above dedupes on its idempotency key, so the retry converges on the
        // cursor instead of double-queueing the run.
        let cursor_id = er_ids::new_v7();
        er_entities::sf_extract_cursor::create::advance(
            ctx.client,
            &cursor_id,
            &ctx.order.org_id,
            &dataset_id,
            &report.source,
            &report.object,
            report.watermark.as_deref(),
            &report.delivery_id,
            report.rows,
        )
        .await
        .map_err(|e| {
            TaskError::Failed(
                er_errors::ErError::stage_failure(format!("advancing the extract cursor: {e}"))
                    .with_class(er_errors::ErrorClass::TransientIo),
            )
        })?;

        Ok(Outcome::Done(
            Plan(SfCursorAdvanced {
                enqueued_job: Some(job.id.clone()),
                watermark: report.watermark.clone(),
            }),
            er_dag::Counters(serde_json::json!({ "rows": report.rows, "enqueued": job.id })),
        ))
    }
}

/// Publish what `materialize_previews` built, then advance the checkpoint
/// (#993). Each proposal is re-validated through the builder and must rebuild
/// to the fingerprint the pod computed; a mismatch fails the task rather than
/// storing something the builder would refuse. Planned previews of an
/// affected entity that no longer forms a group are superseded. The
/// checkpoint moves LAST, by compare-and-set: a failure anywhere before it
/// re-runs the same runs, and publishing is idempotent on the fingerprint.
#[derive(Default)]
pub struct PublishPreviews;

/// Apply a dataset's approved merge previews to Salesforce (#995), journaled
/// per group and per operation — see [`crate::merge_apply`]. Provider
/// failures are group outcomes; only a database failure fails the task.
pub struct ApplyMerges {
    pub egress: crate::urlguard::Egress,
    pub limits: crate::merge_apply::Limits,
}

impl Default for ApplyMerges {
    fn default() -> Self {
        Self {
            egress: crate::urlguard::Egress::Guarded,
            limits: crate::merge_apply::Limits::default(),
        }
    }
}

/// Undo applied merges (#996) — see [`crate::merge_restore`].
pub struct RestoreMerges {
    pub egress: crate::urlguard::Egress,
}

impl Default for RestoreMerges {
    fn default() -> Self {
        Self {
            egress: crate::urlguard::Egress::Guarded,
        }
    }
}

impl ControlTask<PgClient> for RestoreMerges {
    const NAME: &'static str = "restore_merges";
    const VERSION: u32 = 1;
    type Input = ();
    type Output = Plan<crate::merge_restore::Report>;

    async fn run(
        &mut self,
        ctx: &mut ControlCtx<'_, PgClient>,
        (): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let Some(dataset_id) = ctx.order.dataset_id.clone() else {
            return Err(TaskError::Failed(er_errors::ErError::precondition(
                "a merge_restore job must name a dataset".to_string(),
            )));
        };
        let report = crate::merge_restore::run(
            ctx.client,
            &ctx.order.org_id,
            &dataset_id,
            &ctx.order.job_id,
            self.egress,
            crate::merge_apply::Limits::default(),
        )
        .await
        .map_err(|e| {
            TaskError::Failed(
                er_errors::ErError::stage_failure(format!(
                    "merge restore: {}",
                    crate::errfmt::chain(&e)
                ))
                .with_class(er_errors::ErrorClass::TransientIo),
            )
        })?;
        let counters = serde_json::to_value(&report).unwrap_or_default();
        Ok(Outcome::Done(Plan(report), er_dag::Counters(counters)))
    }
}

impl ControlTask<PgClient> for ApplyMerges {
    const NAME: &'static str = "apply_merges";
    const VERSION: u32 = 1;
    type Input = ();
    type Output = Plan<crate::merge_apply::Report>;

    async fn run(
        &mut self,
        ctx: &mut ControlCtx<'_, PgClient>,
        (): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let Some(dataset_id) = ctx.order.dataset_id.clone() else {
            return Err(TaskError::Failed(er_errors::ErError::precondition(
                "a merge_apply job must name a dataset".to_string(),
            )));
        };
        let report = crate::merge_apply::run(
            ctx.client,
            &ctx.order.org_id,
            &dataset_id,
            &ctx.order.job_id,
            self.egress,
            self.limits,
        )
        .await
        .map_err(|e| {
            TaskError::Failed(
                er_errors::ErError::stage_failure(format!(
                    "merge apply: {}",
                    crate::errfmt::chain(&e)
                ))
                .with_class(er_errors::ErrorClass::TransientIo),
            )
        })?;
        let counters = serde_json::to_value(&report).unwrap_or_default();
        Ok(Outcome::Done(Plan(report), er_dag::Counters(counters)))
    }
}

impl ControlTask<PgClient> for PublishPreviews {
    const NAME: &'static str = "publish_previews";
    const VERSION: u32 = 1;
    type Input = (Plan<crate::preview_job::Materialized>,);
    type Output = Plan<crate::dags::plans::PreviewsPublished>;

    async fn run(
        &mut self,
        ctx: &mut ControlCtx<'_, PgClient>,
        (built,): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let built = built.0;
        let failed = |m: String, class: er_errors::ErrorClass| {
            TaskError::Failed(er_errors::ErError::stage_failure(m).with_class(class))
        };
        let Some(dataset_id) = ctx.order.dataset_id.clone() else {
            return Err(TaskError::Failed(er_errors::ErError::precondition(
                "a merge_preview job must name a dataset".to_string(),
            )));
        };
        let Some(next) = built.next.clone() else {
            return Ok(Outcome::NoOp(Plan(crate::dags::plans::PreviewsPublished {
                published: 0,
                superseded: 0,
            })));
        };
        let org_id = ctx.order.org_id.clone();
        // One transaction for the batch and the checkpoint. The executor runs
        // control tasks one at a time on its own client, so nothing else's
        // statements can land inside it.
        ctx.client.batch_execute("BEGIN").await.map_err(|e| {
            failed(
                format!("opening the publish transaction: {e}"),
                er_errors::ErrorClass::TransientIo,
            )
        })?;
        let result = publish_batch(ctx, &org_id, &dataset_id, &built, &next).await;
        let end = if result.is_ok() { "COMMIT" } else { "ROLLBACK" };
        let closed = ctx.client.batch_execute(end).await;
        let published = result?;
        closed.map_err(|e| {
            failed(
                format!("closing the publish transaction: {e}"),
                er_errors::ErrorClass::TransientIo,
            )
        })?;
        Ok(Outcome::Done(
            Plan(published.clone()),
            er_dag::Counters(serde_json::json!({
                "published": published.published,
                "superseded": published.superseded,
            })),
        ))
    }
}

async fn publish_batch(
    ctx: &mut ControlCtx<'_, PgClient>,
    org_id: &str,
    dataset_id: &str,
    built: &crate::preview_job::Materialized,
    next: &crate::preview_job::Checkpoint,
) -> Result<crate::dags::plans::PreviewsPublished, TaskError> {
    let failed = |m: String, class: er_errors::ErrorClass| {
        TaskError::Failed(er_errors::ErError::stage_failure(m).with_class(class))
    };
    let (org_id, dataset_id) = (org_id.to_string(), dataset_id.to_string());
    {
        let expected = crate::pg::preview_watermark(ctx.client, &org_id, &dataset_id)
            .await
            .map_err(|e| {
                failed(
                    format!("reading the checkpoint: {e}"),
                    er_errors::ErrorClass::TransientIo,
                )
            })?;
        let mut published = 0usize;
        let mut kept: std::collections::BTreeSet<(String, String, String)> =
            std::collections::BTreeSet::new();
        for preview in &built.previews {
            let proposal = crate::writeback::PersistedProposalV1::restore(&preview.persisted)
                .map_err(|e| {
                    failed(
                        format!("a built proposal no longer validates: {e}"),
                        er_errors::ErrorClass::Data,
                    )
                })?;
            if proposal.fingerprint() != preview.fingerprint {
                return Err(failed(
                    "a built proposal rebuilt to a different fingerprint".to_string(),
                    er_errors::ErrorClass::Data,
                ));
            }
            let scope = &proposal.content().scope;
            kept.insert((
                scope.entity_id.clone(),
                scope.provider.connection_id.clone(),
                format!("{:?}", scope.provider.object),
            ));
            let publish = crate::pg::PreviewToPublish {
                org_id: &org_id,
                dataset_id: &dataset_id,
                origin_run_id: &preview.origin_run_id,
                policy_id: &preview.policy_id,
                proposal: &proposal,
                persisted: &preview.persisted,
                actor: &format!("job:{}", ctx.order.job_id),
            };
            let stored = crate::pg::publish_preview(ctx.client, &publish)
                .await
                .map_err(|e| {
                    failed(
                        format!("publishing a preview: {e}"),
                        er_errors::ErrorClass::TransientIo,
                    )
                })?;
            if stored.is_some() {
                published += 1;
            }
        }
        // An affected entity with no group under a policy any more: its
        // planned previews are stale.
        let policies = crate::pg::active_writeback_policies(ctx.client, &org_id, &dataset_id)
            .await
            .map_err(|e| {
                failed(
                    format!("reading policies: {e}"),
                    er_errors::ErrorClass::TransientIo,
                )
            })?;
        let mut superseded = 0usize;
        for entity in &built.affected {
            for policy in &policies {
                if kept.contains(&(
                    entity.clone(),
                    policy.salesforce_connection_id.clone(),
                    policy.object.clone(),
                )) {
                    continue;
                }
                let rows = er_entities::merge_preview::update::supersede(
                    ctx.client,
                    &org_id,
                    &dataset_id,
                    &policy.salesforce_connection_id,
                    &policy.object,
                    entity,
                    "",
                )
                .await
                .map_err(|e| {
                    failed(
                        format!("superseding stale previews: {e}"),
                        er_errors::ErrorClass::TransientIo,
                    )
                })?;
                for old in &rows {
                    let _ = er_entities::merge_preview_journal::create::append(
                        ctx.client,
                        &er_ids::new_v7(),
                        &org_id,
                        &dataset_id,
                        &old.id,
                        Some("planned"),
                        "superseded",
                        &old.fingerprint,
                        &format!("job:{}", ctx.order.job_id),
                    )
                    .await;
                }
                superseded += rows.len();
            }
        }
        let moved = crate::pg::advance_preview_watermark(
            ctx.client,
            &org_id,
            &dataset_id,
            expected.as_ref(),
            next,
            built.snapshot,
            &ctx.order.job_id,
        )
        .await
        .map_err(|e| {
            failed(
                format!("advancing the checkpoint: {e}"),
                er_errors::ErrorClass::TransientIo,
            )
        })?;
        if !moved {
            return Err(failed(
                "the merge-preview checkpoint moved underneath this job".to_string(),
                er_errors::ErrorClass::LockConflict,
            ));
        }
        Ok(crate::dags::plans::PreviewsPublished {
            published,
            superseded,
        })
    }
}

/// Queue one `entity.write_back` per entity the pass changed (O4).
///
/// The Postgres half of the write-back: `write_back_scan` read the corrected
/// corpus in a pod, and `webhook::emit` is a database write a pod cannot make
/// (§6.4). This existed only inside the legacy in-process arm, which could do
/// both in one function; on the task path it was absent entirely, so a
/// subscriber integrated against O4 received nothing (#770).
///
/// Best-effort throughout, like the labels and the marks before it: the
/// corrections are applied and marked, so failing here would undo real work to
/// report on it. Every skip is logged — a write-back that silently does not
/// happen is indistinguishable from a tenant whose endpoint is fine.
///
/// The subscriber check happens HERE rather than before the scan, because the
/// list is Postgres. A tenant with no subscriber pays for the scan and nothing
/// else.
#[derive(Default)]
pub struct EmitWriteBack;

impl ControlTask<PgClient> for EmitWriteBack {
    const NAME: &'static str = "emit_write_back";
    const VERSION: u32 = 1;
    type Input = (Plan<crate::dags::plans::WriteBackScanned>,);
    type Output = Plan<crate::dags::plans::WriteBackEmitted>;

    async fn run(
        &mut self,
        ctx: &mut ControlCtx<'_, PgClient>,
        (scanned,): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let entities = scanned.0.entities;
        if entities.is_empty() {
            return Ok(Outcome::NoOp(Plan(crate::dags::plans::WriteBackEmitted {
                queued: 0,
            })));
        }
        // Nothing to do when no subscriber wants it.
        match crate::pg::webhooks_for_delivery(ctx.client, &ctx.order.org_id, "entity.write_back")
            .await
        {
            Ok(targets) if targets.is_empty() => {
                return Ok(Outcome::NoOp(Plan(crate::dags::plans::WriteBackEmitted {
                    queued: 0,
                })));
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(er.run_id = %ctx.order.run_id, error = %e, "the write-back subscriber lookup failed");
                return Ok(Outcome::NoOp(Plan(crate::dags::plans::WriteBackEmitted {
                    queued: 0,
                })));
            }
        }
        let org_name = match crate::pg::get_org_by_id(ctx.client, &ctx.order.org_id).await {
            Ok(Some(org)) => org.name,
            Ok(None) => ctx.order.org_id.clone(),
            Err(e) => {
                tracing::warn!(er.run_id = %ctx.order.run_id, error = %e, "the write-back was skipped: resolving the org failed");
                return Ok(Outcome::NoOp(Plan(crate::dags::plans::WriteBackEmitted {
                    queued: 0,
                })));
            }
        };
        let mut queued = 0i64;
        for entity in &entities {
            let data = serde_json::json!({
                "entity_id": entity.entity_id,
                "run_id": ctx.order.run_id,
                "cause": "review_resolved",
                "master": entity.master,
                "members": entity.members,
            });
            match crate::webhook::emit(
                ctx.client,
                &ctx.order.org_id,
                ctx.order.dataset_id.as_deref(),
                &org_name,
                "entity.write_back",
                &data,
            )
            .await
            {
                Ok(_) => queued += 1,
                Err(e) => {
                    tracing::warn!(er.run_id = %ctx.order.run_id, er.entity_id = %entity.entity_id, error = %e, "the entity.write_back webhook was not queued");
                }
            }
        }
        Ok(Outcome::Done(
            Plan(crate::dags::plans::WriteBackEmitted { queued }),
            er_dag::Counters(serde_json::json!({ "queued": queued })),
        ))
    }
}

/// Creates the tenant workspace and its catalog — the tenant database on the
/// maintenance DSN, or the local lake directory. REAL since Phase 3.
///
/// Explicitly NON-transactional, and that is the design rather than an
/// omission: `CREATE DATABASE` cannot run in a transaction, so the idempotency
/// keys are the `pg_database` probe and `create_dir_all` instead (§3.6, §9.1).
/// A retry over a workspace that already exists is a no-op.
///
/// The provisioning profile comes from the dispatcher's own environment, not
/// from the job: it is deployment configuration, and the job outlives the
/// request that made it — so a profile switched off in between is a stated
/// refusal rather than a panic.
#[derive(Default)]
pub struct CreateCatalog {
    /// The resolved profile, or `None` for "auto-provisioning is not
    /// configured" — which is what `Default` means and what an operator who
    /// set no `ER_PROVISION_MODE` gets.
    profile: Option<crate::provision::Profile>,
    /// A misconfiguration, already rendered: `ProfileError` is not `Clone` and
    /// the DAG builder needs this type to be `Default`, so the message is
    /// carried rather than the error.
    profile_error: Option<String>,
}

impl CreateCatalog {
    /// Build the task from the executor's resolved profile.
    #[must_use]
    pub fn from_profile(
        profile: &Result<Option<crate::provision::Profile>, crate::provision::ProfileError>,
    ) -> Self {
        match profile {
            Ok(p) => CreateCatalog {
                profile: p.clone(),
                profile_error: None,
            },
            Err(e) => CreateCatalog {
                profile: None,
                profile_error: Some(e.to_string()),
            },
        }
    }
}

impl ControlTask<PgClient> for CreateCatalog {
    const NAME: &'static str = "create_catalog";
    const VERSION: u32 = 1;
    type Input = ();
    type Output = Plan<CatalogCreated>;

    async fn run(
        &mut self,
        ctx: &mut ControlCtx<'_, PgClient>,
        (): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let org = crate::pg::get_org_by_id(ctx.client, &ctx.order.org_id)
            .await
            .map_err(|e| {
                TaskError::Failed(
                    er_errors::ErError::stage_failure(format!("reading the org: {e}"))
                        .with_class(er_errors::ErrorClass::TransientIo),
                )
            })?
            .ok_or_else(|| {
                TaskError::Failed(er_errors::ErError::precondition(
                    "the org this job provisions no longer exists".to_string(),
                ))
            })?;
        // From the executor's config, not the environment: see
        // `ExecutorConfig::profile` for why that seam has to exist.
        if let Some(detail) = &self.profile_error {
            return Err(TaskError::Failed(er_errors::ErError::config(format!(
                "auto-provisioning is misconfigured: {detail}"
            ))));
        }
        let profile = self.profile.clone().ok_or_else(|| {
            TaskError::Failed(er_errors::ErError::config(
                "auto-provisioning is not configured on this dispatcher \
                 (ER_PROVISION_MODE)"
                    .to_string(),
            ))
        })?;
        let plan = profile.plan(&org.name);
        crate::provision::create_workspace(&plan, &profile.config_template)
            .map_err(|e| TaskError::Failed(er_errors::ErError::config(e)))?;
        match profile.mode {
            crate::provision::Mode::Local => crate::provision::create_local_lake(&plan)
                .map_err(|e| TaskError::Failed(er_errors::ErError::config(e)))?,
            crate::provision::Mode::Postgres => {
                let database = plan.tenant.as_deref().ok_or_else(|| {
                    TaskError::Failed(er_errors::ErError::config(
                        "postgres mode produced no tenant database name".to_string(),
                    ))
                })?;
                let maint = profile.maint_dsn.as_deref().ok_or_else(|| {
                    TaskError::Failed(er_errors::ErError::config(
                        "postgres mode has no ER_PROVISION_MAINT_DSN".to_string(),
                    ))
                })?;
                crate::dispatcher::create_tenant_database(maint, database)
                    .await
                    .map_err(|e| {
                        TaskError::Failed(
                            er_errors::ErError::stage_failure(e)
                                .with_class(er_errors::ErrorClass::TransientIo),
                        )
                    })?;
            }
        }
        Ok(Outcome::Done(
            Plan(CatalogCreated {
                tenant: plan.tenant.unwrap_or_else(|| org.name.clone()),
            }),
            er_dag::Counters::default(),
        ))
    }
}

/// `set_org_state(provisioning → active)`. REAL since Phase 3.
///
/// A false return is SUCCESS when the org is already active: the transition is
/// guarded on the previous state, so a retry of this task after a partial
/// provision finds the work done rather than failing a tenant that is already
/// serving (§3.6, §9.1).
#[derive(Default)]
pub struct Activate;

impl ControlTask<PgClient> for Activate {
    const NAME: &'static str = "activate";
    const VERSION: u32 = 1;
    type Input = (Plan<LakeInitialized>,);
    type Output = Plan<OrgActivated>;

    async fn run(
        &mut self,
        ctx: &mut ControlCtx<'_, PgClient>,
        _input: Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let org = crate::pg::get_org_by_id(ctx.client, &ctx.order.org_id)
            .await
            .map_err(|e| {
                TaskError::Failed(
                    er_errors::ErError::stage_failure(format!("reading the org: {e}"))
                        .with_class(er_errors::ErrorClass::TransientIo),
                )
            })?
            .ok_or_else(|| {
                TaskError::Failed(er_errors::ErError::precondition(
                    "the org this job provisions no longer exists".to_string(),
                ))
            })?;
        let moved = crate::pg::set_org_state(ctx.client, &org.name, "active", "provisioning")
            .await
            .map_err(|e| {
                TaskError::Failed(
                    er_errors::ErError::stage_failure(format!("activating {}: {e}", org.name))
                        .with_class(er_errors::ErrorClass::TransientIo),
                )
            })?;
        // The default dataset was converged at org creation with the ORG's
        // state of that moment — `provisioning` — and nothing else ever moves
        // it: the boot sweep is DO-NOTHING on existing rows. Flip it with the
        // org, under the same expected-state guard, or every auto-provisioned
        // tenant keeps a dataset the guard rightly renders as still being set
        // up.
        //
        // The legacy arm did this and the task path did not, so a tenant
        // provisioned through the executor read as perpetually setting up.
        // Best-effort and logged, exactly as it was: the org IS active by
        // then, and failing here would retry a provision whose lake exists.
        if moved
            && let Err(e) = crate::pg::set_dataset_state(
                ctx.client,
                &org.id,
                "default",
                "active",
                "provisioning",
            )
            .await
        {
            tracing::warn!(
                er.job_id = %ctx.order.job_id,
                er.org = %org.name,
                error = %e,
                "provisioned the org but its default dataset stayed provisioning"
            );
        }
        // A `dataset_provision` job activates ITS dataset. The org is long
        // active by then, so the branch above never fires for it, and until
        // this arm every dataset a tenant admin created stayed `provisioning`
        // forever — refusing publish and every delivery. The org's default
        // dataset is the org branch's; this one is keyed by the job's own id.
        let mut dataset_activated = false;
        if let Some(dataset_id) = ctx.order.dataset_id.as_deref() {
            match crate::pg::get_dataset_by_id(ctx.client, dataset_id).await {
                Ok(Some(d)) if d.state == "provisioning" => {
                    dataset_activated = crate::pg::set_dataset_state(
                        ctx.client,
                        &org.id,
                        &d.name,
                        "active",
                        "provisioning",
                    )
                    .await
                    .map_err(|e| {
                        TaskError::Failed(
                            er_errors::ErError::stage_failure(format!(
                                "activating dataset {}: {e}",
                                d.name
                            ))
                            .with_class(er_errors::ErrorClass::TransientIo),
                        )
                    })?;
                }
                Ok(_) => {}
                Err(e) => {
                    return Err(TaskError::Failed(
                        er_errors::ErError::stage_failure(format!("reading the dataset: {e}"))
                            .with_class(er_errors::ErrorClass::TransientIo),
                    ));
                }
            }
        }
        Ok(Outcome::Done(
            Plan(OrgActivated {
                activated: moved || dataset_activated,
            }),
            er_dag::Counters(serde_json::json!({
                "already_active": !moved && !dataset_activated,
                "dataset_activated": dataset_activated,
            })),
        ))
    }
}

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
        // `train` found nothing to train on (#1186): nothing to register.
        if trained.model_path.is_empty() {
            return Ok(Outcome::NoOp(Plan(ModelRegistered {
                label: String::new(),
            })));
        }
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
        // The DATASET is the scope these rows live in; the org id is the
        // fallback for a pre-dataset row, which is the idiom the legacy arm
        // used throughout (`dataset_id.unwrap_or(org_id)`). Passing the org id
        // here compiles cleanly and silently matches nothing, because every
        // callee below takes its scope as a bare `&str` named `dataset_id`.
        let scope = ctx.order.dataset_id.as_deref().unwrap_or(&ctx.order.org_id);
        let existing = er_entities::matching_model::read::list(ctx.client, scope)
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

/// Snapshots the pending staged-action ids as `Plan<DrainedActions>`,
/// `init`'s upstream (§3.5). REAL since Phase 3.
///
/// A re-run after a crash re-snapshots, which is correct because nothing
/// downstream has run: the snapshot IS the idempotency boundary, and an action
/// is only marked applied by `settle_actions` at the far end of the pass.
///
/// An empty snapshot is a success, not a refusal: an `apply_corrections`
/// enqueued by a publish that staged nothing has nothing to do, and the
/// downstream tasks skip over an empty set.
#[derive(Default)]
pub struct DrainActions;

impl ControlTask<PgClient> for DrainActions {
    const NAME: &'static str = "drain_actions";
    const VERSION: u32 = 1;
    type Input = ();
    type Output = Plan<DrainedActions>;

    async fn run(
        &mut self,
        ctx: &mut ControlCtx<'_, PgClient>,
        (): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        // The DATASET is the scope these rows live in; the org id is the
        // fallback for a pre-dataset row, which is the idiom the legacy arm
        // used throughout (`dataset_id.unwrap_or(org_id)`). Passing the org id
        // here compiles cleanly and silently matches nothing, because every
        // callee below takes its scope as a bare `&str` named `dataset_id`.
        let scope = ctx.order.dataset_id.as_deref().unwrap_or(&ctx.order.org_id);
        let pending = crate::pg::pending_actions(ctx.client, scope)
            .await
            .map_err(|e| {
                TaskError::Failed(
                    er_errors::ErError::stage_failure(format!("reading staged actions: {e}"))
                        .with_class(er_errors::ErrorClass::TransientIo),
                )
            })?;
        // The ids AND the payloads. `apply_staged` runs in a pod with no
        // control-plane credential (§6.4), so whatever it needs has to travel
        // along this edge; keeping only the ids is what left it applying
        // nothing (#760). The ids stay as their own field because
        // `settle_actions` and the counters read them.
        let action_ids: Vec<String> = pending.iter().map(|a| a.id.clone()).collect();
        let actions: Vec<crate::dags::plans::DrainedAction> = pending
            .into_iter()
            .map(|a| crate::dags::plans::DrainedAction {
                id: a.id,
                action: a.action,
                created_by: a.created_by,
            })
            .collect();
        let count = action_ids.len();
        Ok(Outcome::Done(
            Plan(DrainedActions {
                action_ids,
                actions,
            }),
            er_dag::Counters(serde_json::json!({ "drained": count })),
        ))
    }
}

/// Marks the applied actions applied and records the labels they implied —
/// all Postgres rows, which is why this half exists at all. REAL since
/// Phase 3.
///
/// Takes `apply_staged`'s receipt rather than the drained set: applying stops
/// at the first failing action, so what LANDED is a prefix of what was
/// drained, and marking the rest applied would lose them. It also takes the
/// assembled edge, so the marks happen only once the whole pass succeeded.
///
/// The labels are written best-effort, after the marks and in that order: a
/// label that fails to record is a lost training signal, while a mark that
/// fails to record means the action is applied to the lake and still pending,
/// which re-applies it on the next pass. The worse failure goes first.
#[derive(Default)]
pub struct SettleActions;

impl ControlTask<PgClient> for SettleActions {
    const NAME: &'static str = "settle_actions";
    const VERSION: u32 = 1;
    type Input = (
        Plan<StagedApplied>,
        er_dag::LakeRef<super::edges::Assembled>,
    );
    type Output = Plan<SettledActions>;

    async fn run(
        &mut self,
        ctx: &mut ControlCtx<'_, PgClient>,
        (applied, _assembled): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let receipt = applied.0;
        for id in &receipt.applied {
            crate::pg::mark_action_applied(ctx.client, id)
                .await
                .map_err(|e| {
                    TaskError::Failed(
                        er_errors::ErError::stage_failure(format!(
                            "marking staged action {id} applied: {e}"
                        ))
                        .with_class(er_errors::ErrorClass::TransientIo),
                    )
                })?;
        }
        // Best-effort, and deliberately not fatal: the lake already took these
        // actions, so failing the task here would re-run a pass whose work is
        // done. A missing label costs a training signal, not correctness.
        for label in &receipt.labels {
            if let Err(e) = crate::pg::record_reviewer_label(
                ctx.client,
                &ctx.order.org_id,
                ctx.order.dataset_id.as_deref(),
                (&label.rec_a_key, &label.rec_b_key),
                &label.verdict,
                label.decided_by.as_deref().unwrap_or("reviewer"),
            )
            .await
            {
                tracing::warn!(
                    er.record_a = %label.rec_a_key,
                    er.record_b = %label.rec_b_key,
                    error = %e,
                    "the reviewer label was not recorded"
                );
            }
        }
        // `review.resolved` (O3c), for the resolutions that APPLIED.
        //
        // Deliberately not emitted by the resolve endpoint: a resolution is
        // staged and drained later, so an event fired at staging time would
        // tell a subscriber about an intent the engine has not enforced — they
        // would read a corpus that disagrees with the event they just got.
        //
        // This lived only in the legacy arm, so deleting that arm left the
        // event catalogued with no emit site anywhere. `pg_webhook_catalog.rs`
        // is what noticed, which is the gate earning its keep: a webhook that
        // stops being sent is invisible from the inside.
        //
        // Best-effort and after the marks, like the labels above and for the
        // same reason: the decisions are in the lake and the corpus has been
        // corrected, so failing here would undo real work to report on it.
        if !receipt.resolutions.is_empty() {
            let org_name = match crate::pg::get_org_by_id(ctx.client, &ctx.order.org_id).await {
                Ok(Some(org)) => org.name,
                Ok(None) => ctx.order.org_id.clone(),
                Err(e) => {
                    tracing::warn!(er.run_id = %ctx.order.run_id, error = %e, "review.resolved was skipped: resolving the org failed");
                    String::new()
                }
            };
            if !org_name.is_empty() {
                for (review_id, resolution) in &receipt.resolutions {
                    if let Err(e) = crate::webhook::emit(
                        ctx.client,
                        &ctx.order.org_id,
                        ctx.order.dataset_id.as_deref(),
                        &org_name,
                        "review.resolved",
                        &serde_json::json!({
                            "review_id": review_id,
                            "resolution": resolution,
                            "run_id": ctx.order.run_id,
                        }),
                    )
                    .await
                    {
                        tracing::warn!(er.review_id = %review_id, error = %e, "the review.resolved webhook was not queued");
                    }
                }
            }
        }
        let applied_count = i64::try_from(receipt.applied.len()).unwrap_or(i64::MAX);
        Ok(Outcome::Done(
            Plan(SettledActions {
                applied: applied_count,
            }),
            er_dag::Counters(serde_json::json!({
                "applied": applied_count,
                "labels": receipt.labels.len(),
                "resolutions": receipt.resolutions.len(),
            })),
        ))
    }
}

/// Reads the approval tag, stages a resolve action per approved review, and
/// enqueues the `apply_corrections` that merges them. REAL since Phase 3.
///
/// Never opens a lake — it reads tags and writes staged actions, both
/// Postgres, and the merging is the job it enqueues (§3.5). That is exactly
/// why `apply_approved` is the one kind with no pod task at all.
///
/// Capped per tick rather than unbounded: a tenant that tags a thousand groups
/// should see them merged over several ticks instead of one job holding the
/// org's single writer slot for an hour. The tag is removed as each review is
/// staged, which is what makes the remainder get picked up next tick and what
/// makes this task idempotent — a second run with nothing newly approved
/// stages nothing.
///
/// One unstageable review must not strand the rest, so a staging failure is
/// logged and skipped rather than failing the task: the tag stays on it and
/// the next tick retries exactly that one.
#[derive(Default)]
pub struct StageApproved;

impl ControlTask<PgClient> for StageApproved {
    const NAME: &'static str = "stage_approved";
    const VERSION: u32 = 1;
    type Input = ();
    type Output = Plan<StagedApproved>;

    async fn run(
        &mut self,
        ctx: &mut ControlCtx<'_, PgClient>,
        (): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let org = crate::pg::get_org_by_id(ctx.client, &ctx.order.org_id)
            .await
            .map_err(|e| {
                TaskError::Failed(
                    er_errors::ErError::stage_failure(format!("reading the org: {e}"))
                        .with_class(er_errors::ErrorClass::TransientIo),
                )
            })?
            .ok_or_else(|| {
                TaskError::Failed(er_errors::ErError::precondition(
                    "the org this job serves no longer exists".to_string(),
                ))
            })?;
        // The published config names the approval tag. Reading a YAML file is
        // not opening a lake, so this stays a control task.
        let config = load_published_config(&ctx.order.lake_env)?;
        let name = config.approval.tag.trim().to_string();
        if name.is_empty() {
            // The ordinary case for a tenant that has not configured the gate.
            return Ok(Outcome::NoOp(Plan(StagedApproved {
                staged: 0,
                enqueued: false,
            })));
        }
        let Some(tag) = crate::pg::tag_by_name(ctx.client, &ctx.order.org_id, &name)
            .await
            .map_err(|e| {
                TaskError::Failed(
                    er_errors::ErError::stage_failure(format!("reading tag '{name}': {e}"))
                        .with_class(er_errors::ErrorClass::TransientIo),
                )
            })?
        else {
            return Ok(Outcome::NoOp(Plan(StagedApproved {
                staged: 0,
                enqueued: false,
            })));
        };
        // The DATASET is the scope these rows live in; the org id is the
        // fallback for a pre-dataset row, which is the idiom the legacy arm
        // used throughout (`dataset_id.unwrap_or(org_id)`). Passing the org id
        // here compiles cleanly and silently matches nothing, because every
        // callee below takes its scope as a bare `&str` named `dataset_id`.
        // `tag_by_name` above is correctly org-scoped — the tag VOCABULARY
        // belongs to the org — while tag APPLICATION is dataset-scoped.
        let scope = ctx.order.dataset_id.as_deref().unwrap_or(&ctx.order.org_id);
        let approved = crate::pg::subjects_for_tag(
            ctx.client,
            scope,
            &tag.id,
            Some("review"),
            crate::dispatcher::MAX_APPROVED,
        )
        .await
        .map_err(|e| {
            TaskError::Failed(
                er_errors::ErError::stage_failure(format!("reading approved reviews: {e}"))
                    .with_class(er_errors::ErrorClass::TransientIo),
            )
        })?;

        let actor = format!("schedule:approval:{name}");
        let mut staged = 0usize;
        for tagging in &approved {
            let action = serde_json::json!({
                "type": "resolve_review",
                "review_id": tagging.subject_id,
                "resolution": "match",
            });
            if let Err(e) = crate::pg::stage_action(
                ctx.client,
                &ctx.order.org_id,
                ctx.order.dataset_id.as_deref(),
                &action,
                &actor,
            )
            .await
            {
                tracing::warn!(
                    er.review_id = %tagging.subject_id,
                    er.org = %org.name,
                    error = %e,
                    "staging the approved review failed"
                );
                continue;
            }
            if let Err(e) =
                crate::pg::unapply_tag(ctx.client, scope, &tag.id, "review", &tagging.subject_id)
                    .await
            {
                // Staged but still tagged: the next tick would stage it again.
                // Said out loud rather than swallowed — a duplicate staged
                // resolve is a no-op on apply but noise in the ledger.
                tracing::warn!(
                    er.review_id = %tagging.subject_id,
                    error = %e,
                    "the review was staged but its tag could not be removed"
                );
            }
            staged += 1;
        }

        let mut enqueued = false;
        if staged > 0 {
            let params = serde_json::json!({ "skip_ingest": true });
            let idem = format!("approval:{}:{}", ctx.order.org_id, approved.len());
            crate::pg::enqueue(
                ctx.client,
                &ctx.order.org_id,
                ctx.order.dataset_id.as_deref(),
                "apply_corrections",
                0,
                Some(&idem),
                Some(&params),
                crate::pg::Provenance::actor(&actor),
            )
            .await
            .map_err(|e| {
                TaskError::Failed(
                    er_errors::ErError::stage_failure(format!(
                        "enqueueing the approved apply: {e}"
                    ))
                    .with_class(er_errors::ErrorClass::TransientIo),
                )
            })?;
            enqueued = true;
        }
        let count = i64::try_from(staged).unwrap_or(i64::MAX);
        Ok(Outcome::Done(
            Plan(StagedApproved {
                staged: count,
                enqueued,
            }),
            er_dag::Counters(serde_json::json!({ "staged": count })),
        ))
    }
}

/// The tenant's published config, from the path its env names.
///
/// Deliberately a plain file read: a control task may not open a lake (§3.5),
/// and a YAML file is not one.
fn load_published_config(lake_env: &serde_json::Value) -> Result<er_config::Config, TaskError> {
    let path = lake_env
        .get("ER_CONFIG_PATH")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            TaskError::Failed(er_errors::ErError::config(
                "org env missing ER_CONFIG_PATH".to_string(),
            ))
        })?;
    let text = std::fs::read_to_string(path).map_err(|e| {
        TaskError::Failed(er_errors::ErError::config(format!(
            "read config {path}: {e}"
        )))
    })?;
    er_config::load_config_str(&text)
        .map_err(|e| TaskError::Failed(er_errors::ErError::config(crate::errfmt::chain(&e))))
}
