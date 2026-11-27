//! Applying approved Lead conversions to Salesforce (#520), journaled.
//!
//! Per approved proposal: `APPROVED → APPLYING` at most once (a unique
//! index), then both records are re-read (`queryAll`):
//!
//! - a Lead already converted INTO the proposed Contact (by a person, a Flow)
//!   is `CONVERTED_EXTERNALLY`; converted into anything else, or deleted, is
//!   `FAILED` — conversion is not reversible, so it is reported, never retried;
//! - a Lead or Contact whose `SystemModstamp` moved since the snapshot is
//!   `SKIPPED_STALE`;
//! - otherwise the Contact's Account is read, the call is marked IN FLIGHT,
//!   and `convertLead()` is sent. A call found in flight on resume is
//!   reconciled by re-reading the Lead (`IsConverted`, `ConvertedContactId`),
//!   never replayed blind.
//!
//! After a conversion, a FULL extract of the Lead object is queued: extraction
//! excludes converted Leads, so the refresh tombstones it and the next run
//! drops it from the entity; an incremental Contact extract brings the
//! Contact's post-conversion values into the golden record.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_postgres::Client;

use crate::merge_apply::{ApplyFatal, Limits};
use crate::sf_apply::{self, CallError, Org};
use crate::urlguard::Egress;

/// What one job did.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub converted: u64,
    pub converted_externally: u64,
    pub skipped_stale: u64,
    pub failed: u64,
    pub deferred: u64,
    pub extracts_queued: u64,
    pub stopped: Option<String>,
}

enum End {
    Settled(&'static str, String),
    Deferred(Option<&'static str>),
}

fn stop_reason(e: &CallError) -> Option<&'static str> {
    match e {
        CallError::RateLimited(_) => Some("rate_limited"),
        CallError::BudgetExhausted => Some("budget"),
        _ => None,
    }
}

fn text(m: &serde_json::Map<String, serde_json::Value>, k: &str) -> Option<String> {
    m.get(k).and_then(|v| v.as_str()).map(str::to_string)
}

fn flag(m: &serde_json::Map<String, serde_json::Value>, k: &str) -> bool {
    m.get(k)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Salesforce ids compare on their 15-character case-sensitive prefix.
fn same_id(a: &str, b: &str) -> bool {
    a.get(..15).is_some_and(|x| b.get(..15) == Some(x))
}

/// Run one conversion pass over a dataset.
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
        er_entities::lead_conversion::LeadConversion,
        Option<er_entities::lead_conversion_apply::LeadConversionApply>,
    )> = Vec::new();
    for open in er_entities::lead_conversion_apply::read::open(
        client,
        org_id,
        dataset_id,
        limits.max_groups,
    )
    .await?
    {
        if let Some(p) = er_entities::lead_conversion::read::by_id(
            client,
            &open.lead_conversion_id,
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
        for p in
            er_entities::lead_conversion::read::ready_to_apply(client, org_id, dataset_id, room)
                .await?
        {
            work.push((p, None));
        }
    }
    let mut orgs: HashMap<String, Option<Org>> = HashMap::new();
    let mut statuses: HashMap<String, String> = HashMap::new();
    let mut calls = 0usize;
    for (proposal, open) in work {
        if crate::merge_apply::job_canceling(client, org_id, job_id).await? {
            report.stopped = Some("cancelled".into());
            break;
        }
        if calls >= limits.max_operations {
            report.stopped = Some("operation_cap".into());
            break;
        }
        let sf = if let Some(o) = orgs.get(&proposal.salesforce_connection_id) {
            o.clone()
        } else {
            let o = crate::merge_apply::delegated(
                client,
                org_id,
                &proposal.salesforce_connection_id,
                egress,
                limits,
            )
            .await?
            .ok();
            orgs.insert(proposal.salesforce_connection_id.clone(), o.clone());
            o
        };
        let Some(sf) = sf else {
            report.deferred += 1;
            continue;
        };
        let apply = if let Some(a) = open {
            a
        } else {
            let inserted = er_entities::lead_conversion_apply::create::begin(
                client,
                &er_ids::new_v7(),
                job_id,
                &proposal.id,
                org_id,
                dataset_id,
                &proposal.fingerprint,
                "approved",
            )
            .await?;
            match er_entities::lead_conversion_apply::read::by_conversion(
                client,
                org_id,
                dataset_id,
                &proposal.id,
            )
            .await?
            .into_iter()
            .next()
            {
                Some(a) if inserted == 1 => a,
                _ => continue,
            }
        };
        calls += 1;
        let status = if let Some(s) = statuses.get(&proposal.salesforce_connection_id) {
            s.clone()
        } else {
            match sf_apply::converted_status(&sf).await {
                Ok(s) => {
                    statuses.insert(proposal.salesforce_connection_id.clone(), s.clone());
                    s
                }
                Err(CallError::Rejected(code)) => {
                    settle(
                        client,
                        &apply,
                        "failed",
                        &format!("no converted Lead status ({code})"),
                    )
                    .await?;
                    report.failed += 1;
                    continue;
                }
                Err(e) => {
                    report.deferred += 1;
                    if let Some(why) = stop_reason(&e) {
                        report.stopped = Some(why.into());
                        break;
                    }
                    continue;
                }
            }
        };
        match convert_one(client, &sf, &proposal, &apply, &status).await? {
            End::Settled(state, detail) => {
                settle(client, &apply, state, &detail).await?;
                match state {
                    "converted" | "converted_externally" => {
                        if state == "converted" {
                            report.converted += 1;
                        } else {
                            report.converted_externally += 1;
                        }
                        report.extracts_queued += resync(client, &proposal).await?;
                    }
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

async fn settle(
    client: &Client,
    apply: &er_entities::lead_conversion_apply::LeadConversionApply,
    status: &str,
    detail: &str,
) -> Result<(), ApplyFatal> {
    er_entities::lead_conversion_apply::update::settle(
        client,
        status,
        Some(detail),
        &apply.id,
        &apply.org_id,
    )
    .await?;
    Ok(())
}

async fn convert_one(
    client: &Client,
    sf: &Org,
    p: &er_entities::lead_conversion::LeadConversion,
    apply: &er_entities::lead_conversion_apply::LeadConversionApply,
    status: &str,
) -> Result<End, ApplyFatal> {
    let lead = match sf_apply::record(
        sf,
        "Lead",
        &p.lead_external_id,
        &[
            "Id",
            "SystemModstamp",
            "IsDeleted",
            "IsConverted",
            "ConvertedContactId",
        ],
    )
    .await
    {
        Ok(Some(l)) => l,
        Ok(None) => return Ok(End::Settled("failed", "the Lead no longer exists".into())),
        Err(CallError::Rejected(code)) => {
            return Ok(End::Settled(
                "failed",
                format!("reading the Lead refused: {code}"),
            ));
        }
        Err(e) => return Ok(End::Deferred(stop_reason(&e))),
    };
    if flag(&lead, "IsConverted") {
        let into = text(&lead, "ConvertedContactId").unwrap_or_default();
        return Ok(if same_id(&into, &p.contact_external_id) {
            if apply.call_state == "in_flight" {
                End::Settled("converted", "verified by re-reading the Lead".into())
            } else {
                End::Settled(
                    "converted_externally",
                    "the Lead was already converted into this Contact".into(),
                )
            }
        } else {
            End::Settled(
                "failed",
                "the Lead was converted into a different Contact; conversion is not reversible"
                    .into(),
            )
        });
    }
    if flag(&lead, "IsDeleted") {
        return Ok(End::Settled("failed", "the Lead was deleted".into()));
    }
    let contact = match sf_apply::record(
        sf,
        "Contact",
        &p.contact_external_id,
        &["Id", "SystemModstamp", "IsDeleted", "AccountId"],
    )
    .await
    {
        Ok(Some(c)) if !flag(&c, "IsDeleted") => c,
        Ok(_) => {
            return Ok(End::Settled(
                "failed",
                "the target Contact no longer exists".into(),
            ));
        }
        Err(CallError::Rejected(code)) => {
            return Ok(End::Settled(
                "failed",
                format!("reading the Contact refused: {code}"),
            ));
        }
        Err(e) => return Ok(End::Deferred(stop_reason(&e))),
    };
    // An in-flight call whose Lead is not converted did not land: send again.
    if apply.call_state != "in_flight" {
        if text(&lead, "SystemModstamp").as_deref() != Some(p.lead_modstamp.as_str()) {
            return Ok(End::Settled(
                "skipped_stale",
                format!("{} changed since sync", p.lead_record_key),
            ));
        }
        if text(&contact, "SystemModstamp").as_deref() != Some(p.contact_modstamp.as_str()) {
            return Ok(End::Settled(
                "skipped_stale",
                format!("{} changed since sync", p.contact_record_key),
            ));
        }
    }
    let Some(account) = text(&contact, "AccountId") else {
        return Ok(End::Settled(
            "failed",
            "the Contact has no Account to convert into".into(),
        ));
    };
    er_entities::lead_conversion_apply::update::call(
        client,
        Some(&account),
        &apply.id,
        &apply.org_id,
    )
    .await?;
    match sf_apply::convert_lead(
        sf,
        &p.lead_external_id,
        &p.contact_external_id,
        &account,
        status,
    )
    .await
    {
        Ok(()) => Ok(End::Settled("converted", "converted".into())),
        Err(CallError::Ambiguous(_)) => Ok(End::Deferred(None)),
        Err(
            e @ (CallError::RateLimited(_) | CallError::BudgetExhausted | CallError::Transient(_)),
        ) => Ok(End::Deferred(stop_reason(&e))),
        Err(CallError::Rejected(code)) => Ok(End::Settled(
            "failed",
            format!("convertLead refused: {code}"),
        )),
        Err(e) => Ok(End::Settled("failed", e.to_string())),
    }
}

/// Queue a FULL Lead extract (the converted Lead is omitted, so ingest
/// tombstones it) and an incremental Contact extract, for every source the
/// connection delivers those objects into. Returns how many were queued.
async fn resync(
    client: &Client,
    p: &er_entities::lead_conversion::LeadConversion,
) -> Result<u64, ApplyFatal> {
    let sources = er_entities::source_delivery_provenance::read::for_dataset(
        client,
        &p.org_id,
        &p.dataset_id,
    )
    .await?;
    let mut queued = 0;
    for (object, mode) in [("Lead", "full"), ("Contact", "incremental")] {
        let Some(source) = sources
            .iter()
            .filter(|r| {
                r.salesforce_connection_id == p.salesforce_connection_id && r.object == object
            })
            .map(|r| r.source.clone())
            .min()
        else {
            continue;
        };
        let params = json!({"connection_id": p.salesforce_connection_id, "object": object, "source": source, "mode": mode});
        // One resync per (object, conversion): repeated conversions in one
        // pass each name their own, and a retry of the same one converges.
        let key = format!("lead-convert-resync:{}:{object}:{}", p.dataset_id, p.id);
        crate::pg::enqueue(
            client,
            &p.org_id,
            Some(&p.dataset_id),
            "extract",
            0,
            Some(&key),
            Some(&params),
            crate::pg::Provenance::actor("job:lead_convert"),
        )
        .await?;
        queued += 1;
    }
    Ok(queued)
}
