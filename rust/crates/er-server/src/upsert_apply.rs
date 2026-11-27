//! Applying approved upserts to Salesforce (#998), journaled.
//!
//! Per approved preview: `APPROVED → APPLYING` at most once (a unique index),
//! then the record is read by its External ID. A record modified after the
//! proposal was made is `SKIPPED_STALE`, never overwritten. Otherwise the
//! preimage (the record's current values, or `null` for a create) is written
//! and the call marked IN FLIGHT before the upsert is sent.
//!
//! A call found in flight on resume is reconciled by reading the record by
//! its key: holding the proposed values, it landed; otherwise the upsert is
//! sent again — safe, because an upsert keyed by External ID addresses the
//! same record every time and cannot create a second one.
//!
//! A CREATED record is then asserted always-match to a member of its entity,
//! so the next extraction ingests it into that entity instead of resolving a
//! new one: extraction converges on what writeback wrote.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_postgres::Client;

use crate::merge_apply::{ApplyFatal, Limits};
use crate::sf_apply::{self, CallError, Org, Upserted};
use crate::urlguard::Egress;

/// What one job did.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub created: u64,
    pub updated: u64,
    pub skipped_stale: u64,
    pub failed: u64,
    pub deferred: u64,
    pub assertions_staged: u64,
    pub stopped: Option<String>,
}

enum End {
    Settled(&'static str, Option<String>, String),
    Deferred(Option<&'static str>),
}

fn stop_reason(e: &CallError) -> Option<&'static str> {
    match e {
        CallError::RateLimited(_) => Some("rate_limited"),
        CallError::BudgetExhausted => Some("budget"),
        _ => None,
    }
}

/// Run one upsert apply pass over a dataset.
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
    let mut work: Vec<(
        er_entities::upsert_preview::UpsertPreview,
        Option<er_entities::upsert_apply::UpsertApply>,
    )> = Vec::new();
    for open in
        er_entities::upsert_apply::read::open(client, org_id, dataset_id, limits.max_groups).await?
    {
        if let Some(p) = er_entities::upsert_preview::read::by_id(
            client,
            &open.upsert_preview_id,
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
        for p in er_entities::upsert_preview::read::ready_to_apply(client, org_id, dataset_id, room)
            .await?
        {
            work.push((p, None));
        }
    }
    let mut orgs: HashMap<String, Option<Org>> = HashMap::new();
    let mut calls = 0usize;
    for (preview, open) in work {
        if crate::merge_apply::job_canceling(client, org_id, job_id).await? {
            report.stopped = Some("cancelled".into());
            break;
        }
        if calls >= limits.max_operations {
            report.stopped = Some("operation_cap".into());
            break;
        }
        let sf = if let Some(o) = orgs.get(&preview.salesforce_connection_id) {
            o.clone()
        } else {
            let o = crate::merge_apply::delegated(
                client,
                org_id,
                &preview.salesforce_connection_id,
                egress,
                limits,
            )
            .await?
            .ok();
            orgs.insert(preview.salesforce_connection_id.clone(), o.clone());
            o
        };
        let Some(sf) = sf else {
            report.deferred += 1;
            continue;
        };
        let apply = if let Some(a) = open {
            a
        } else {
            {
                let inserted = er_entities::upsert_apply::create::begin(
                    client,
                    &er_ids::new_v7(),
                    job_id,
                    &preview.id,
                    org_id,
                    dataset_id,
                    &preview.fingerprint,
                    "approved",
                )
                .await?;
                let begun = er_entities::upsert_apply::read::by_preview(
                    client,
                    org_id,
                    dataset_id,
                    &preview.id,
                )
                .await?
                .into_iter()
                .next();
                match begun {
                    Some(a) if inserted == 1 => a,
                    _ => continue,
                }
            }
        };
        calls += 1;
        match apply_one(client, &sf, &preview, &apply).await? {
            End::Settled(status, sf_id, detail) => {
                er_entities::upsert_apply::update::settle(
                    client,
                    status,
                    sf_id.as_deref(),
                    Some(&detail),
                    &apply.id,
                    &apply.org_id,
                )
                .await?;
                match status {
                    "created" => {
                        report.created += 1;
                        if let Some(id) = &sf_id
                            && converge(client, &preview, id).await?
                        {
                            report.assertions_staged += 1;
                        }
                    }
                    "updated" => report.updated += 1,
                    "skipped_stale" => report.skipped_stale += 1,
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

/// The proposal's provider fields → values.
fn proposed(preview: &er_entities::upsert_preview::UpsertPreview) -> BTreeMap<String, Value> {
    serde_json::from_value(preview.fields.clone()).unwrap_or_default()
}

fn modified_after(modstamp: &str, at: &chrono::DateTime<chrono::Utc>) -> bool {
    chrono::DateTime::parse_from_str(modstamp, "%Y-%m-%dT%H:%M:%S%.f%z")
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(modstamp))
        .is_ok_and(|m| m.with_timezone(&chrono::Utc) > *at)
}

async fn apply_one(
    client: &Client,
    sf: &Org,
    preview: &er_entities::upsert_preview::UpsertPreview,
    apply: &er_entities::upsert_apply::UpsertApply,
) -> Result<End, ApplyFatal> {
    let fields = proposed(preview);
    if fields.is_empty() {
        return Ok(End::Settled(
            "failed",
            None,
            "the proposal writes no fields".into(),
        ));
    }
    let names: Vec<String> = fields.keys().cloned().collect();
    let found = match sf_apply::find_by_external_id(
        sf,
        &preview.object,
        &preview.external_id_field,
        &preview.external_id_value,
        &names,
    )
    .await
    {
        Ok(f) => f,
        Err(CallError::Rejected(code)) => {
            return Ok(End::Settled(
                "failed",
                None,
                format!("reading by external id refused: {code}"),
            ));
        }
        Err(e) => return Ok(End::Deferred(stop_reason(&e))),
    };

    if apply.call_state == "in_flight" {
        // Reconcile, never assume: the values are there or they are not.
        if let Some(f) = &found
            && f.fields == fields
        {
            let status = if apply.preimage == "null" {
                "created"
            } else {
                "updated"
            };
            return Ok(End::Settled(
                status,
                Some(f.id.clone()),
                "verified by reading the record back".into(),
            ));
        }
    } else {
        if let Some(f) = &found
            && modified_after(&f.system_modstamp, &preview.created_at)
        {
            return Ok(End::Settled(
                "skipped_stale",
                Some(f.id.clone()),
                "the record was modified after the proposal; re-sync and regenerate".into(),
            ));
        }
        let preimage = found.as_ref().map_or_else(
            || "null".to_string(),
            |f| json!({"id": f.id, "fields": f.fields}).to_string(),
        );
        er_entities::upsert_apply::update::call(client, &preimage, &apply.id, &apply.org_id)
            .await?;
    }

    match sf_apply::upsert(
        sf,
        &preview.object,
        &preview.external_id_field,
        &preview.external_id_value,
        &fields,
    )
    .await
    {
        Ok(Upserted::Created(id)) => Ok(End::Settled("created", Some(id), "created".into())),
        Ok(Upserted::Updated(id)) => {
            let id = if id.is_empty() {
                found.map(|f| f.id)
            } else {
                Some(id)
            };
            Ok(End::Settled("updated", id, "updated".into()))
        }
        // In flight: the next run reads it back by its key.
        Err(CallError::Ambiguous(_)) => Ok(End::Deferred(None)),
        Err(
            e @ (CallError::RateLimited(_) | CallError::BudgetExhausted | CallError::Transient(_)),
        ) => Ok(End::Deferred(stop_reason(&e))),
        Err(CallError::Rejected(code)) => Ok(End::Settled(
            "failed",
            None,
            format!("upsert refused: {code}"),
        )),
        Err(e) => Ok(End::Settled("failed", None, e.to_string())),
    }
}

/// Assert the created record always-match to its entity's anchor member, so
/// the next extraction joins the entity rather than resolving a new one.
/// `false` when no source is mapped to the connection's object yet.
async fn converge(
    client: &Client,
    preview: &er_entities::upsert_preview::UpsertPreview,
    sf_id: &str,
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
        .min()
    else {
        return Ok(false);
    };
    let action = json!({
        "type": "add_assertion",
        "kind": "always",
        "a": preview.anchor_record_key,
        "b": format!("{source}:{sf_id}"),
        "created_by": "job:upsert_apply",
        "note": format!("salesforce-upsert:{}", preview.id),
    });
    crate::pg::stage_action(
        client,
        &preview.org_id,
        Some(&preview.dataset_id),
        &action,
        "job:upsert_apply",
    )
    .await?;
    Ok(true)
}
