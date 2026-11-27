//! Applying approved merge previews to Salesforce, journaled (#995).
//!
//! Per preview: `APPROVED → APPLYING` is one conditional insert (a unique
//! index makes it at most once), then
//!
//! 1. the proposal is restored through `MergeProposal::build` and must carry
//!    the approved fingerprint — the apply executes exactly what was approved;
//! 2. every member is re-read (`queryAll`, so merged-away rows are visible);
//!    an operation a crash left IN FLIGHT is resolved from that read, never
//!    replayed blind;
//! 3. a member whose `SystemModstamp` moved since the snapshot makes the group
//!    `SKIPPED_STALE` and queues an incremental extract, whose run the next
//!    preview generation re-scores;
//! 4. a victim already merged into the master (deleted, `MasterRecordId` = the
//!    master) is external-merge evidence; every victim so → `APPLIED_EXTERNALLY`.
//!    A missing victim, or one merged elsewhere, is `FAILED` — absence alone
//!    proves nothing;
//! 5. the preimage is written, then the master update and each merge batch are
//!    journaled in flight before the call and settled after it.
//!
//! A group's failure never stops the batch. A rate limit or an exhausted API
//! budget stops the job, leaving the in-progress apply `APPLYING` for the next
//! run to resume. Cancellation is checked between groups.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;
use serde_json::{Value, json};
use tokio_postgres::Client;

use crate::sf_apply::{self, CallError, Live, Org};
use crate::sf_oauth::{self, AccessError};
use crate::urlguard::Egress;
use crate::writeback::PersistedProposalV1;

/// Per-job bounds: the control task blocks the executor while it runs.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_groups: i64,
    pub max_operations: usize,
    pub backoff: std::time::Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_groups: 25,
            max_operations: 100,
            backoff: std::time::Duration::from_secs(1),
        }
    }
}

/// What a job did.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Report {
    pub applied: u64,
    pub applied_externally: u64,
    pub skipped_stale: u64,
    pub failed: u64,
    /// Left APPLYING (ambiguous, rate-limited, budget) or not begun because
    /// the connection has no usable authorization.
    pub deferred: u64,
    pub operations: u64,
    pub resyncs_queued: u64,
    /// Why the job stopped early, if it did: `cancelled`, `rate_limited`,
    /// `budget`, `operation_cap`.
    pub stopped: Option<String>,
}

/// A failure that is the database's, not a group's.
#[derive(Debug, thiserror::Error)]
pub enum ApplyFatal {
    #[error("the apply journal could not be written")]
    Db(#[from] tokio_postgres::Error),
}

/// How one group ended, or why it did not.
enum GroupEnd {
    Settled(&'static str, String),
    /// Left APPLYING; `Some` stops the whole job with that reason.
    Deferred(Option<&'static str>),
}

const ACTOR: &str = "job:merge_apply";

/// Run one apply pass over a dataset.
///
/// # Errors
/// Only database failures; every provider failure is a group outcome.
pub async fn run(
    client: &Client,
    org_id: &str,
    dataset_id: &str,
    job_id: &str,
    egress: Egress,
    limits: Limits,
) -> Result<Report, ApplyFatal> {
    let mut report = Report::default();
    let mut orgs: HashMap<String, Result<Org, String>> = HashMap::new();
    let mut work: Vec<(
        er_entities::merge_preview::MergePreview,
        Option<er_entities::merge_apply::MergeApply>,
    )> = Vec::new();
    for open in
        er_entities::merge_apply::read::open(client, org_id, dataset_id, limits.max_groups).await?
    {
        if let Some(p) = er_entities::merge_preview::read::by_id(
            client,
            &open.merge_preview_id,
            org_id,
            dataset_id,
        )
        .await?
        .into_iter()
        .next()
        {
            work.push((p, Some(open)));
        }
    }
    let room = limits.max_groups - i64::try_from(work.len()).unwrap_or(i64::MAX);
    if room > 0 {
        for p in er_entities::merge_preview::read::ready_to_apply(client, org_id, dataset_id, room)
            .await?
        {
            work.push((p, None));
        }
    }
    let mut ops_used = 0usize;
    for (preview, open) in work {
        if job_canceling(client, org_id, job_id).await? {
            report.stopped = Some("cancelled".into());
            break;
        }
        if ops_used >= limits.max_operations {
            report.stopped = Some("operation_cap".into());
            break;
        }
        // Authorization first: a preview is not begun through a connection
        // that cannot act.
        let sf = if let Some(o) = orgs.get(&preview.salesforce_connection_id) {
            o.clone()
        } else {
            let o = delegated(
                client,
                org_id,
                &preview.salesforce_connection_id,
                egress,
                limits,
            )
            .await?;
            orgs.insert(preview.salesforce_connection_id.clone(), o.clone());
            o
        };
        let Ok(sf) = sf else {
            report.deferred += 1;
            continue;
        };
        let apply = match open {
            Some(a) => a,
            None => match begin(client, &preview, job_id).await? {
                Some(a) => a,
                None => continue,
            },
        };
        let end = apply_group(client, &sf, &preview, &apply, &mut ops_used, &mut report).await?;
        match end {
            GroupEnd::Settled(status, detail) => {
                settle(client, &preview, &apply, status, &detail).await?;
                match status {
                    "applied" => report.applied += 1,
                    "applied_externally" => report.applied_externally += 1,
                    "skipped_stale" => {
                        report.skipped_stale += 1;
                        if queue_resync(client, &preview, job_id).await? {
                            report.resyncs_queued += 1;
                        }
                    }
                    _ => report.failed += 1,
                }
            }
            GroupEnd::Deferred(stop) => {
                report.deferred += 1;
                if let Some(why) = stop {
                    report.stopped = Some(why.into());
                    break;
                }
            }
        }
    }
    report.operations = ops_used as u64;
    Ok(report)
}

async fn job_canceling(client: &Client, org_id: &str, job_id: &str) -> Result<bool, ApplyFatal> {
    Ok(crate::pg::get_job(client, org_id, job_id)
        .await?
        .is_some_and(|j| j.state == "canceling" || j.state == "canceled"))
}

/// A delegated session for the connection, or why there is none. Attended:
/// the job was requested by a principal through the API.
async fn delegated(
    client: &Client,
    org_id: &str,
    connection_id: &str,
    egress: Egress,
    limits: Limits,
) -> Result<Result<Org, String>, ApplyFatal> {
    let Some(conn) =
        crate::pg::salesforce_connection_credential(client, org_id, connection_id).await?
    else {
        return Ok(Err("no connection".into()));
    };
    match sf_oauth::access_token(client, org_id, connection_id, false, egress).await {
        Ok(a) => Ok(Ok(Org {
            instance_url: a.instance_url,
            access_token: a.access_token,
            api_version: conn.api_version,
            egress,
            backoff: limits.backoff,
        })),
        Err(AccessError::Db(e)) => Err(e.into()),
        Err(e) => {
            tracing::warn!(error = %e, "merge apply: connection has no usable delegated authorization");
            Ok(Err(e.to_string()))
        }
    }
}

async fn journal(
    client: &Client,
    preview: &er_entities::merge_preview::MergePreview,
    from: &str,
    to: &str,
) -> Result<(), ApplyFatal> {
    er_entities::merge_preview_journal::create::append(
        client,
        &er_ids::new_v7(),
        &preview.org_id,
        &preview.dataset_id,
        &preview.id,
        Some(from),
        to,
        &preview.fingerprint,
        ACTOR,
    )
    .await?;
    Ok(())
}

/// APPROVED → APPLYING, or `None` when someone else got there first.
async fn begin(
    client: &Client,
    preview: &er_entities::merge_preview::MergePreview,
    job_id: &str,
) -> Result<Option<er_entities::merge_apply::MergeApply>, ApplyFatal> {
    client.batch_execute("BEGIN").await?;
    let inserted = er_entities::merge_apply::create::begin(
        client,
        &er_ids::new_v7(),
        job_id,
        &preview.id,
        &preview.org_id,
        &preview.dataset_id,
        &preview.fingerprint,
        "approved",
    )
    .await;
    let inserted = match inserted {
        Ok(n) => n,
        Err(e) => {
            client.batch_execute("ROLLBACK").await?;
            return Err(e.into());
        }
    };
    if inserted == 1
        && let Err(e) = journal(client, preview, "approved", "applying").await
    {
        client.batch_execute("ROLLBACK").await?;
        return Err(e);
    }
    client.batch_execute("COMMIT").await?;
    Ok(er_entities::merge_apply::read::by_preview(
        client,
        &preview.org_id,
        &preview.dataset_id,
        &preview.id,
    )
    .await?
    .into_iter()
    .next()
    .filter(|a| inserted == 1 && a.status == "applying"))
}

async fn settle(
    client: &Client,
    preview: &er_entities::merge_preview::MergePreview,
    apply: &er_entities::merge_apply::MergeApply,
    status: &str,
    detail: &str,
) -> Result<(), ApplyFatal> {
    client.batch_execute("BEGIN").await?;
    let result = async {
        let rows = er_entities::merge_apply::update::settle(
            client,
            status,
            Some(detail),
            &apply.id,
            &apply.org_id,
        )
        .await?;
        if !rows.is_empty() {
            journal(client, preview, "applying", status).await?;
        }
        Ok::<_, ApplyFatal>(())
    }
    .await;
    client
        .batch_execute(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })
        .await?;
    result
}

/// Queue an incremental extract of the stale group's object, so its new
/// revision is ingested and the next preview generation re-scores it.
async fn queue_resync(
    client: &Client,
    preview: &er_entities::merge_preview::MergePreview,
    job_id: &str,
) -> Result<bool, ApplyFatal> {
    let sources = er_entities::source_delivery_provenance::read::for_dataset(
        client,
        &preview.org_id,
        &preview.dataset_id,
    )
    .await?;
    let Some(source) = sources
        .iter()
        .filter(|r| {
            r.salesforce_connection_id == preview.salesforce_connection_id
                && r.object == preview.object
        })
        .map(|r| r.source.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .next()
    else {
        return Ok(false);
    };
    let params = json!({
        "connection_id": preview.salesforce_connection_id,
        "object": preview.object,
        "source": source,
        "mode": "incremental",
    });
    let key = format!(
        "merge-resync:{}:{}:{}:{job_id}",
        preview.dataset_id, preview.salesforce_connection_id, preview.object
    );
    crate::pg::enqueue(
        client,
        &preview.org_id,
        Some(&preview.dataset_id),
        "extract",
        0,
        Some(&key),
        Some(&params),
        crate::pg::Provenance::actor(ACTOR),
    )
    .await?;
    Ok(true)
}

fn stop_reason(e: &CallError) -> Option<&'static str> {
    match e {
        CallError::RateLimited(_) => Some("rate_limited"),
        CallError::BudgetExhausted => Some("budget"),
        _ => None,
    }
}

/// Run one group to an end. Database failures propagate; provider ones are
/// the group's.
async fn apply_group(
    client: &Client,
    sf: &Org,
    preview: &er_entities::merge_preview::MergePreview,
    apply: &er_entities::merge_apply::MergeApply,
    ops_used: &mut usize,
    report: &mut Report,
) -> Result<GroupEnd, ApplyFatal> {
    let _ = report;
    let proposal = match PersistedProposalV1::restore(&preview.proposal) {
        Ok(p) if p.fingerprint() == apply.fingerprint => p,
        Ok(_) => {
            return Ok(GroupEnd::Settled(
                "failed",
                "proposal_fingerprint_mismatch".into(),
            ));
        }
        Err(_) => return Ok(GroupEnd::Settled("failed", "proposal_unrestorable".into())),
    };
    let content = proposal.content();
    let object = preview.object.as_str();
    let master = content.master.external_id.clone();
    let victims: Vec<&crate::writeback::MemberSnapshot> = content.victims.iter().collect();
    let mut ids: Vec<String> = vec![master.clone()];
    ids.extend(victims.iter().map(|v| v.external_id.clone()));

    let live = match sf_apply::live_state(sf, object, &ids).await {
        Ok(l) => l,
        Err(e) => return Ok(GroupEnd::Deferred(stop_reason(&e))),
    };
    let merged_into_master = |l: &Live| {
        l.is_deleted
            && l.master_record_id
                .as_deref()
                .is_some_and(|m| same_id(m, &master))
    };

    // Resolve anything a crash left in flight, from what Salesforce now says.
    let mut ops =
        er_entities::merge_apply_operation::read::by_apply(client, &apply.org_id, &apply.id)
            .await?;
    for op in ops.iter_mut().filter(|o| o.status == "in_flight") {
        let landed = match op.kind.as_str() {
            "merge" => {
                let ids: Vec<String> = serde_json::from_str(&op.payload).unwrap_or_default();
                if ids
                    .iter()
                    .all(|id| live.get(id).is_some_and(merged_into_master))
                {
                    Some(true)
                } else if ids.iter().all(|id| {
                    let snap = victims.iter().find(|v| &v.external_id == id);
                    live.get(id).is_some_and(|l| {
                        !l.is_deleted
                            && snap.is_some_and(|s| s.system_modstamp == l.system_modstamp)
                    })
                }) {
                    Some(false)
                } else {
                    None
                }
            }
            _ => match live.get(&master) {
                Some(l) if l.system_modstamp == content.master.system_modstamp => Some(false),
                _ => None,
            },
        };
        let (status, outcome) = match landed {
            Some(true) => ("done", "verified"),
            Some(false) => ("failed", "not_landed"),
            None => {
                return Ok(GroupEnd::Settled(
                    "failed",
                    format!("ambiguous_{}_unresolved", op.kind),
                ));
            }
        };
        er_entities::merge_apply_operation::update::finish(
            client,
            status,
            outcome,
            &apply.id,
            op.seq,
            &apply.org_id,
        )
        .await?;
        op.status = status.to_string();
    }
    let done = |kind: &'static str| {
        ops.iter()
            .filter(move |o| o.status == "done" && o.kind == kind)
    };
    let master_touched = ops.iter().any(|o| o.status == "done");
    let mut merged: BTreeSet<String> = done("merge")
        .flat_map(|o| serde_json::from_str::<Vec<String>>(&o.payload).unwrap_or_default())
        .collect();
    let master_updated = content.field_changes.is_empty() || done("update_master").next().is_some();

    // Staleness and external-merge evidence.
    match live.get(&master) {
        None => return Ok(GroupEnd::Settled("failed", "master_not_found".into())),
        Some(l) if l.is_deleted => return Ok(GroupEnd::Settled("failed", "master_deleted".into())),
        Some(l) if !master_touched && l.system_modstamp != content.master.system_modstamp => {
            return Ok(GroupEnd::Settled(
                "skipped_stale",
                format!("member {} changed since sync", content.master.record_key),
            ));
        }
        Some(_) => {}
    }
    let mut external = 0usize;
    for v in &victims {
        if merged.contains(&v.external_id) {
            continue;
        }
        match live.get(&v.external_id) {
            None => {
                return Ok(GroupEnd::Settled(
                    "failed",
                    format!("member {} not found; no merge evidence", v.record_key),
                ));
            }
            Some(l) if merged_into_master(l) => {
                external += 1;
                merged.insert(v.external_id.clone());
            }
            Some(l) if l.is_deleted => {
                return Ok(GroupEnd::Settled(
                    "failed",
                    format!("member {} was merged into another record", v.record_key),
                ));
            }
            Some(l) if l.system_modstamp != v.system_modstamp => {
                return Ok(GroupEnd::Settled(
                    "skipped_stale",
                    format!("member {} changed since sync", v.record_key),
                ));
            }
            Some(_) => {}
        }
    }
    if external == victims.len() && !master_touched {
        return Ok(GroupEnd::Settled(
            "applied_externally",
            "every victim is already merged into the master".into(),
        ));
    }

    // The preimage, once, before the first mutation.
    if apply.preimage.is_empty() {
        let members: Vec<Value> = std::iter::once(&content.master)
            .chain(victims.iter().copied())
            .map(|m| json!({"external_id": m.external_id, "record_key": m.record_key, "system_modstamp": m.system_modstamp, "fields": m.fields}))
            .collect();
        let live_stamps: BTreeMap<&String, &String> =
            live.iter().map(|(k, l)| (k, &l.system_modstamp)).collect();
        let preimage = json!({"members": members, "live_modstamps": live_stamps}).to_string();
        er_entities::merge_apply::update::preimage(client, &preimage, &apply.id, &apply.org_id)
            .await?;
    }

    let mut seq = i64::try_from(ops.len()).unwrap_or(i64::MAX);
    if !master_updated {
        let fields: BTreeMap<String, Value> = content
            .field_changes
            .iter()
            .map(|(k, c)| (k.clone(), c.after.clone()))
            .collect();
        let payload = serde_json::to_string(&fields).unwrap_or_default();
        match operate(
            client,
            apply,
            seq,
            "update_master",
            &payload,
            ops_used,
            sf_apply::update(sf, object, &master, &fields),
        )
        .await?
        {
            Ok(()) => seq += 1,
            Err(end) => return Ok(end),
        }
    }
    let remaining: Vec<String> = victims
        .iter()
        .map(|v| v.external_id.clone())
        .filter(|id| !merged.contains(id))
        .collect();
    for batch in remaining.chunks(sf_apply::MERGE_BATCH) {
        let payload = serde_json::to_string(batch).unwrap_or_default();
        match operate(
            client,
            apply,
            seq,
            "merge",
            &payload,
            ops_used,
            sf_apply::merge(sf, object, &master, batch),
        )
        .await?
        {
            Ok(()) => seq += 1,
            Err(end) => return Ok(end),
        }
    }
    Ok(GroupEnd::Settled(
        "applied",
        format!("{} victim(s) merged", victims.len() - external),
    ))
}

/// Journal an operation in flight, make the call, settle it.
async fn operate(
    client: &Client,
    apply: &er_entities::merge_apply::MergeApply,
    seq: i64,
    kind: &str,
    payload: &str,
    ops_used: &mut usize,
    call: impl std::future::Future<Output = Result<(), CallError>>,
) -> Result<Result<(), GroupEnd>, ApplyFatal> {
    er_entities::merge_apply_operation::create::start(
        client,
        &er_ids::new_v7(),
        &apply.org_id,
        &apply.dataset_id,
        &apply.id,
        seq,
        kind,
        payload,
    )
    .await?;
    *ops_used += 1;
    let finish = |status: &'static str, outcome: String| async move {
        er_entities::merge_apply_operation::update::finish(
            client,
            status,
            &outcome,
            &apply.id,
            seq,
            &apply.org_id,
        )
        .await
        .map(|_| ())
    };
    match call.await {
        Ok(()) => {
            finish("done", "ok".into()).await?;
            Ok(Ok(()))
        }
        // Left IN FLIGHT: the next run resolves it by re-reading.
        Err(CallError::Ambiguous(_)) => Ok(Err(GroupEnd::Deferred(None))),
        Err(e @ (CallError::RateLimited(_) | CallError::BudgetExhausted)) => {
            finish("failed", "not_processed".into()).await?;
            Ok(Err(GroupEnd::Deferred(stop_reason(&e))))
        }
        Err(CallError::Rejected(code)) => {
            finish("failed", code.clone()).await?;
            Ok(Err(GroupEnd::Settled(
                "failed",
                format!("{kind} rejected: {code}"),
            )))
        }
        Err(e) => {
            finish("failed", "not_processed".into()).await?;
            Ok(Err(GroupEnd::Settled("failed", format!("{kind}: {e}"))))
        }
    }
}

/// Salesforce ids compare on their 15-character case-sensitive prefix.
fn same_id(a: &str, b: &str) -> bool {
    a.get(..15).is_some_and(|x| b.get(..15) == Some(x))
}
