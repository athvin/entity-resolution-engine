//! Undoing an applied merge (#996), from the apply's journal.
//!
//! Composition, per undo:
//!
//! 1. **Resolution side** — a never-match assertion between the master and
//!    each victim, staged exactly as the engine's own unmerge does, so the
//!    next resolution does not re-propose the merge.
//! 2. **Victims** — each merged victim is undeleted from the Recycle Bin.
//!    When the bin no longer has it (emptied or expired), it is recreated
//!    from the apply's preimage and the step records old id → new id. A victim
//!    already live (a repeated undo, a hand restore) is left alone.
//! 3. **Master** — each field the apply wrote is reverted to its preimage
//!    value ONLY where Salesforce still holds the value the apply wrote. A
//!    field changed since is a conflict, never overwritten.
//!
//! Every step is journaled in flight before its call, so a retry after a
//! partial restore resumes rather than repeats. A recreate whose response
//! was lost cannot be told apart from one that never ran without risking a
//! duplicate, so it is recorded as a conflict rather than retried.
//!
//! Records the merge reparented to the master (opportunities, cases,
//! activities…) are NOT moved back; the view says so.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_postgres::Client;

use crate::merge_apply::{ApplyFatal, Limits};
use crate::sf_apply::{self, CallError, Org};
use crate::urlguard::Egress;

const ACTOR: &str = "job:merge_restore";

/// What one restore job did.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub restored: u64,
    pub partial: u64,
    pub failed: u64,
    pub deferred: u64,
    pub stopped: Option<String>,
}

/// The apply's preimage, as `merge_apply` wrote it.
#[derive(Deserialize)]
struct Preimage {
    members: Vec<PreMember>,
}

#[derive(Deserialize)]
struct PreMember {
    external_id: String,
    record_key: String,
    #[serde(default)]
    fields: BTreeMap<String, Value>,
}

enum End {
    Settled(&'static str, Vec<Value>, String),
    Deferred(Option<&'static str>),
}

/// Run every open undo of a dataset.
///
/// # Errors
/// Only database failures.
pub async fn run(
    client: &Client,
    org_id: &str,
    dataset_id: &str,
    job_id: &str,
    egress: Egress,
    limits: Limits,
) -> Result<Report, ApplyFatal> {
    let mut report = Report::default();
    for restore in
        er_entities::merge_restore::read::open(client, org_id, dataset_id, limits.max_groups)
            .await?
    {
        if crate::merge_apply::job_canceling(client, org_id, job_id).await? {
            report.stopped = Some("cancelled".into());
            break;
        }
        let Some(apply) = er_entities::merge_apply::read::by_preview(
            client,
            org_id,
            dataset_id,
            &restore.merge_preview_id,
        )
        .await?
        .into_iter()
        .next() else {
            continue;
        };
        let Some(preview) = er_entities::merge_preview::read::by_id(
            client,
            &restore.merge_preview_id,
            org_id,
            dataset_id,
        )
        .await?
        .into_iter()
        .next() else {
            continue;
        };
        let Ok(sf) = crate::merge_apply::delegated(
            client,
            org_id,
            &preview.salesforce_connection_id,
            egress,
            limits,
        )
        .await?
        else {
            report.deferred += 1;
            continue;
        };
        match restore_one(client, &sf, &preview, &apply, &restore).await? {
            End::Settled(status, conflicts, detail) => {
                settle(client, &preview, &restore, status, &conflicts, &detail).await?;
                match status {
                    "restored" => report.restored += 1,
                    "partial" => report.partial += 1,
                    _ => report.failed += 1,
                }
            }
            End::Deferred(stop) => {
                report.deferred += 1;
                if let Some(why) = stop {
                    report.stopped = Some(why.into());
                    break;
                }
            }
        }
    }
    Ok(report)
}

async fn settle(
    client: &Client,
    preview: &er_entities::merge_preview::MergePreview,
    restore: &er_entities::merge_restore::MergeRestore,
    status: &str,
    conflicts: &[Value],
    detail: &str,
) -> Result<(), ApplyFatal> {
    client.batch_execute("BEGIN").await?;
    let result = async {
        let rows = er_entities::merge_restore::update::settle(
            client,
            status,
            &Value::Array(conflicts.to_vec()).to_string(),
            Some(detail),
            &restore.id,
            &restore.org_id,
        )
        .await?;
        if !rows.is_empty() {
            er_entities::merge_preview_journal::create::append(
                client,
                &er_ids::new_v7(),
                &preview.org_id,
                &preview.dataset_id,
                &preview.id,
                Some("restoring"),
                status,
                &preview.fingerprint,
                ACTOR,
            )
            .await?;
        }
        Ok::<_, ApplyFatal>(())
    }
    .await;
    client
        .batch_execute(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })
        .await?;
    result
}

struct Steps<'a> {
    client: &'a Client,
    restore: &'a er_entities::merge_restore::MergeRestore,
    rows: Vec<er_entities::merge_restore_step::MergeRestoreStep>,
}

impl Steps<'_> {
    fn done(&self, kind: &str, target: &str) -> bool {
        self.rows
            .iter()
            .any(|s| s.kind == kind && s.target_id == target && s.status == "done")
    }

    async fn start(&mut self, kind: &str, target: &str) -> Result<i64, ApplyFatal> {
        let seq = i64::try_from(self.rows.len()).unwrap_or(i64::MAX);
        er_entities::merge_restore_step::create::start(
            self.client,
            &er_ids::new_v7(),
            &self.restore.org_id,
            &self.restore.dataset_id,
            &self.restore.id,
            seq,
            kind,
            target,
        )
        .await?;
        self.rows = er_entities::merge_restore_step::read::by_restore(
            self.client,
            &self.restore.org_id,
            &self.restore.id,
        )
        .await?;
        Ok(seq)
    }

    async fn finish(
        &mut self,
        seq: i64,
        status: &str,
        outcome: &str,
        new_id: Option<&str>,
    ) -> Result<(), ApplyFatal> {
        er_entities::merge_restore_step::update::finish(
            self.client,
            status,
            outcome,
            new_id,
            &self.restore.id,
            seq,
            &self.restore.org_id,
        )
        .await?;
        if let Some(r) = self.rows.iter_mut().find(|r| r.seq == seq) {
            r.status = status.to_string();
            r.outcome = Some(outcome.to_string());
            r.new_id = new_id.map(str::to_string);
        }
        Ok(())
    }
}

fn stop_reason(e: &CallError) -> Option<&'static str> {
    match e {
        CallError::RateLimited(_) => Some("rate_limited"),
        CallError::BudgetExhausted => Some("budget"),
        _ => None,
    }
}

#[allow(clippy::too_many_lines)] // one linear compensation script reads best in one place
async fn restore_one(
    client: &Client,
    sf: &Org,
    preview: &er_entities::merge_preview::MergePreview,
    apply: &er_entities::merge_apply::MergeApply,
    restore: &er_entities::merge_restore::MergeRestore,
) -> Result<End, ApplyFatal> {
    let Ok(pre) = serde_json::from_str::<Preimage>(&apply.preimage) else {
        return Ok(End::Settled(
            "failed",
            Vec::new(),
            "the apply has no preimage to restore from".into(),
        ));
    };
    let object = preview.object.as_str();
    let ops = er_entities::merge_apply_operation::read::by_apply(client, &apply.org_id, &apply.id)
        .await?;
    let merged: Vec<String> = ops
        .iter()
        .filter(|o| o.kind == "merge" && o.status == "done")
        .flat_map(|o| serde_json::from_str::<Vec<String>>(&o.payload).unwrap_or_default())
        .collect();
    let written: BTreeMap<String, Value> = ops
        .iter()
        .find(|o| o.kind == "update_master" && o.status == "done")
        .and_then(|o| serde_json::from_str(&o.payload).ok())
        .unwrap_or_default();
    let Some(master) = pre.members.first() else {
        return Ok(End::Settled(
            "failed",
            Vec::new(),
            "the preimage names no master".into(),
        ));
    };
    let mut steps = Steps {
        client,
        restore,
        rows: er_entities::merge_restore_step::read::by_restore(
            client,
            &restore.org_id,
            &restore.id,
        )
        .await?,
    };
    let mut conflicts: Vec<Value> = Vec::new();

    // 1. Resolution: never-match the master and each merged victim.
    for victim in pre
        .members
        .iter()
        .filter(|m| merged.contains(&m.external_id))
    {
        if steps.done("never_assert", &victim.record_key) {
            continue;
        }
        let seq = steps.start("never_assert", &victim.record_key).await?;
        let action = json!({
            "type": "add_assertion",
            "kind": "never",
            "a": master.record_key,
            "b": victim.record_key,
            "created_by": restore.requested_by,
            "note": format!("salesforce-restore:{}", preview.id),
        });
        crate::pg::stage_action(
            client,
            &restore.org_id,
            Some(&restore.dataset_id),
            &action,
            &restore.requested_by,
        )
        .await?;
        steps.finish(seq, "done", "staged", None).await?;
    }

    // 2. Victims: undelete, else recreate from the preimage.
    let mut ids: Vec<String> = vec![master.external_id.clone()];
    ids.extend(merged.iter().cloned());
    let live = match sf_apply::live_state(sf, object, &ids).await {
        Ok(l) => l,
        Err(e) => return Ok(End::Deferred(stop_reason(&e))),
    };
    // A step a crash left in flight.
    let in_flight: Vec<(i64, String, String)> = steps
        .rows
        .iter()
        .filter(|s| s.status == "in_flight")
        .map(|s| (s.seq, s.kind.clone(), s.target_id.clone()))
        .collect();
    for (seq, kind, target) in in_flight {
        let present = live.get(&target).is_some_and(|l| !l.is_deleted);
        match kind.as_str() {
            "undelete" if present => steps.finish(seq, "done", "verified", None).await?,
            "undelete" | "revert_master" => steps.finish(seq, "failed", "not_landed", None).await?,
            _ => {
                steps.finish(seq, "failed", "outcome_unknown", None).await?;
                conflicts.push(json!({"record": target, "reason": "a recreate's outcome is unknown; check Salesforce for a duplicate before retrying"}));
            }
        }
    }
    for victim in pre
        .members
        .iter()
        .filter(|m| merged.contains(&m.external_id))
    {
        let id = &victim.external_id;
        if steps.done("undelete", id) || steps.done("recreate", id) {
            continue;
        }
        if steps
            .rows
            .iter()
            .any(|s| s.kind == "recreate" && &s.target_id == id)
        {
            continue; // an unknown recreate: already a conflict, never repeated
        }
        match live.get(id) {
            Some(l) if !l.is_deleted => {
                let seq = steps.start("undelete", id).await?;
                steps.finish(seq, "done", "already_present", None).await?;
                continue;
            }
            Some(_) => {
                let seq = steps.start("undelete", id).await?;
                match sf_apply::undelete(sf, std::slice::from_ref(id)).await {
                    Ok(r) if r.first().is_some_and(Result::is_ok) => {
                        steps.finish(seq, "done", "ok", None).await?;
                        continue;
                    }
                    Ok(r) => {
                        let code = r
                            .into_iter()
                            .next()
                            .and_then(Result::err)
                            .unwrap_or_default();
                        steps.finish(seq, "failed", &code, None).await?;
                    }
                    Err(CallError::Ambiguous(_)) => return Ok(End::Deferred(None)),
                    Err(e) => {
                        steps.finish(seq, "failed", "not_processed", None).await?;
                        if stop_reason(&e).is_some() {
                            return Ok(End::Deferred(stop_reason(&e)));
                        }
                    }
                }
            }
            None => {}
        }
        // The Recycle Bin no longer has it: recreate from the preimage.
        let seq = steps.start("recreate", id).await?;
        match sf_apply::create(sf, object, &victim.fields).await {
            Ok(new_id) => steps.finish(seq, "done", "ok", Some(&new_id)).await?,
            Err(CallError::Ambiguous(_)) => {
                steps.finish(seq, "failed", "outcome_unknown", None).await?;
                conflicts.push(json!({"record": id, "reason": "a recreate's outcome is unknown; check Salesforce for a duplicate before retrying"}));
            }
            Err(e @ (CallError::RateLimited(_) | CallError::BudgetExhausted)) => {
                steps.finish(seq, "failed", "not_processed", None).await?;
                return Ok(End::Deferred(stop_reason(&e)));
            }
            Err(e) => {
                let code = match &e {
                    CallError::Rejected(c) => c.clone(),
                    other => other.to_string(),
                };
                steps.finish(seq, "failed", &code, None).await?;
                conflicts.push(
                    json!({"record": id, "reason": format!("could not be restored ({code})")}),
                );
            }
        }
    }

    // 3. Master: revert what the apply wrote, where it is still what we wrote.
    if !written.is_empty() && !steps.done("revert_master", &master.external_id) {
        match live.get(&master.external_id) {
            Some(l) if !l.is_deleted => {
                let fields: Vec<String> = written.keys().cloned().collect();
                let current = match sf_apply::read_fields(sf, object, &master.external_id, &fields).await {
                    Ok(c) => c,
                    Err(e) => return Ok(End::Deferred(stop_reason(&e))),
                };
                let mut revert = BTreeMap::new();
                for (field, wrote) in &written {
                    let before = master.fields.get(field).cloned().unwrap_or(Value::Null);
                    if current.get(field) == Some(&before) {
                        continue; // already reverted (a retry, or by hand)
                    }
                    if current.get(field) == Some(wrote) {
                        revert.insert(field.clone(), master.fields.get(field).cloned().unwrap_or(Value::Null));
                    } else {
                        conflicts.push(json!({"record": master.external_id, "field": field, "reason": "changed since the merge; not overwritten"}));
                    }
                }
                if !revert.is_empty() {
                    let seq = steps.start("revert_master", &master.external_id).await?;
                    match sf_apply::update(sf, object, &master.external_id, &revert).await {
                        Ok(()) => steps.finish(seq, "done", "ok", None).await?,
                        Err(CallError::Ambiguous(_)) => return Ok(End::Deferred(None)),
                        Err(e) => {
                            steps.finish(seq, "failed", "not_processed", None).await?;
                            if stop_reason(&e).is_some() {
                                return Ok(End::Deferred(stop_reason(&e)));
                            }
                            conflicts.push(json!({"record": master.external_id, "reason": format!("master revert refused ({e})")}));
                        }
                    }
                }
            }
            _ => conflicts.push(json!({"record": master.external_id, "reason": "the master no longer exists; its fields were not reverted"})),
        }
    }

    let victims_back: BTreeSet<&String> = steps
        .rows
        .iter()
        .filter(|s| (s.kind == "undelete" || s.kind == "recreate") && s.status == "done")
        .map(|s| &s.target_id)
        .collect();
    let status = if conflicts.is_empty() {
        "restored"
    } else if victims_back.is_empty() && !merged.is_empty() {
        "failed"
    } else {
        "partial"
    };
    Ok(End::Settled(
        status,
        conflicts,
        format!(
            "{} of {} victim(s) restored; related records the merge reparented were not moved back",
            victims_back.len(),
            merged.len()
        ),
    ))
}
