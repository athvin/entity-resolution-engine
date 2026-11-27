//! Merge-preview materialization (#993): turn a dataset's resolution results
//! into writeback proposals, one per (entity, connection, object) group.
//!
//! The pod half reads ONE pinned lake snapshot and the Postgres context the
//! work order carries (the pod cannot reach Postgres): active writeback
//! policies, every run's delivery provenance, the ambiguous sources and the
//! checkpoint. It emits proposals as persisted inputs; the control half
//! re-validates each through `MergeProposal::build`, publishes, and only then
//! advances the checkpoint.
//!
//! A member may join a group only on evidence: its latest landed row is not
//! deleted, that row's run carries provenance (#1296), the provenance names
//! the member's own source, the source is not delivered by two connections,
//! and a policy exists for the (connection, object). A row an upload landed —
//! even one whose id collides with a Salesforce id — has no provenance and so
//! never joins. Customer data flows through here: nothing is logged.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::writeback::{
    MemberSnapshot, MergeObject, MergeProposal, PersistedProposalV1, ProposalInput, ProposalScope,
    ProviderScope,
};

/// Runs one job reads at most.
pub const MAX_RUNS: i64 = 20;
/// Groups one job proposes at most; it stops at a run boundary past this.
pub const MAX_GROUPS: usize = 200;

/// One active policy, as the work order carries it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyCtx {
    pub id: String,
    pub connection_id: String,
    pub object: String,
    pub election_policy: String,
    pub writeback_allowlist: Vec<String>,
    pub field_mapping: BTreeMap<String, String>,
}

/// Where one run's records came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceCtx {
    pub source: String,
    pub connection_id: String,
    pub object: String,
}

/// The `(snapshot_end, run_id)` checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub snapshot_end: i64,
    pub run_id: String,
}

/// The Postgres context the resolver puts in the work order.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PreviewContext {
    #[serde(default)]
    pub watermark: Option<Checkpoint>,
    #[serde(default)]
    pub policies: Vec<PolicyCtx>,
    /// run id → provenance.
    #[serde(default)]
    pub provenance: BTreeMap<String, ProvenanceCtx>,
    #[serde(default)]
    pub ambiguous: BTreeSet<String>,
}

/// One proposal the pod built, persisted-shaped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltPreview {
    /// The group's weakest-link match evidence (#997); `None` when absent.
    #[serde(default)]
    pub score: Option<f64>,
    pub origin_run_id: String,
    pub policy_id: String,
    pub fingerprint: String,
    pub persisted: Value,
}

/// What a materialization produced.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Materialized {
    pub snapshot: i64,
    /// Where the checkpoint moves to once these are published; `None` when no
    /// run was read.
    pub next: Option<Checkpoint>,
    pub previews: Vec<BuiltPreview>,
    /// Every entity the read runs changed, so planned previews of an entity
    /// that no longer forms a group can be superseded.
    pub affected: Vec<String>,
    pub excluded: BTreeMap<String, u64>,
}

fn object_of(name: &str) -> Option<MergeObject> {
    match name {
        "Lead" => Some(MergeObject::Lead),
        "Contact" => Some(MergeObject::Contact),
        "Account" => Some(MergeObject::Account),
        _ => None,
    }
}

/// A group's identity: (entity id, connection id, object).
type GroupKey = (String, String, String);

const SEP: &str = "\u{1f}";
const KEY_SEP: &str = "\u{1e}";

/// The golden record's survivable values, by column name — what a mapping's
/// keys may name.
pub const GOLDEN_COLUMNS: [&str; 11] = [
    "given_name",
    "family_name",
    "email",
    "phone_e164",
    "addr_number",
    "addr_street",
    "addr_unit",
    "addr_city",
    "addr_region",
    "addr_postal",
    "birth_date",
];

fn golden_values(row: &er_entities::golden_record::read::ByIdsRow) -> BTreeMap<String, Value> {
    let s = |v: &Option<String>| v.clone().map_or(Value::Null, Value::String);
    BTreeMap::from([
        ("given_name".into(), s(&row.given_name)),
        ("family_name".into(), s(&row.family_name)),
        ("email".into(), s(&row.email)),
        ("phone_e164".into(), s(&row.phone_e164)),
        ("addr_number".into(), s(&row.addr_number)),
        ("addr_street".into(), s(&row.addr_street)),
        ("addr_unit".into(), s(&row.addr_unit)),
        ("addr_city".into(), s(&row.addr_city)),
        ("addr_region".into(), s(&row.addr_region)),
        ("addr_postal".into(), s(&row.addr_postal)),
        (
            "birth_date".into(),
            row.birth_date
                .map_or(Value::Null, |d| Value::String(d.to_string())),
        ),
    ])
}

/// Read the pinned snapshot and build proposals.
///
/// # Errors
/// A lake read or the election query failing, as text.
#[allow(clippy::too_many_lines)]
pub fn materialize(
    session: &dedupe_core::Session,
    org_id: &str,
    dataset_id: &str,
    config: &er_config::Config,
    ctx: &PreviewContext,
) -> Result<Materialized, String> {
    let conn = session.connection();
    let snapshot = crate::read::latest_snapshot(session)?;
    let mut out = Materialized {
        snapshot,
        ..Materialized::default()
    };
    let bump = |reason: &str, out: &mut Materialized, n: u64| {
        *out.excluded.entry(reason.to_string()).or_default() += n;
    };
    let (after_snapshot, after_run) = ctx
        .watermark
        .as_ref()
        .map_or((-1, String::new()), |w| (w.snapshot_end, w.run_id.clone()));
    let runs = er_entities::run::read::succeeded_after(
        conn,
        snapshot,
        after_snapshot,
        &after_run,
        MAX_RUNS,
    )
    .map_err(|e| e.to_string())?;
    if runs.is_empty() {
        return Ok(out);
    }
    let policies: BTreeMap<(String, String), &PolicyCtx> = ctx
        .policies
        .iter()
        .map(|p| ((p.connection_id.clone(), p.object.clone()), p))
        .collect();
    let mut protect: Option<bool> = None;

    for run in &runs {
        let Some(snapshot_end) = run.snapshot_end else {
            continue;
        };
        let affected: Vec<String> =
            er_entities::entity_event::read::affected_in_runs(conn, snapshot, &run.id)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|r| r.entity_id)
                .collect();
        if !affected.is_empty() && !policies.is_empty() {
            let members = er_entities::entity_membership::read::members_of(
                conn,
                snapshot,
                &affected.join(SEP),
            )
            .map_err(|e| e.to_string())?;
            let keys: Vec<String> = members
                .iter()
                .map(|m| format!("{}{KEY_SEP}{}", m.source_system, m.source_record_id))
                .collect();
            let raw: BTreeMap<(String, String), er_entities::raw_record::read::LatestForKeysRow> =
                er_entities::raw_record::read::latest_for_keys(conn, snapshot, &keys.join(SEP))
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .map(|r| ((r.source_system.clone(), r.source_record_id.clone()), r))
                    .collect();
            // (entity, connection, object) → eligible member snapshots.
            let mut groups: BTreeMap<GroupKey, Vec<MemberSnapshot>> = BTreeMap::new();
            for m in &members {
                let Some(row) = raw.get(&(m.source_system.clone(), m.source_record_id.clone()))
                else {
                    bump("no_raw_row", &mut out, 1);
                    continue;
                };
                if row.is_deleted {
                    bump("deleted", &mut out, 1);
                    continue;
                }
                let Some(prov) = ctx.provenance.get(&row.ingest_batch_id) else {
                    bump("no_provenance", &mut out, 1);
                    continue;
                };
                if prov.source != m.source_system {
                    bump("provenance_source_mismatch", &mut out, 1);
                    continue;
                }
                if ctx.ambiguous.contains(&m.source_system) {
                    bump("ambiguous_provenance", &mut out, 1);
                    continue;
                }
                let Some(policy) = policies.get(&(prov.connection_id.clone(), prov.object.clone()))
                else {
                    bump("no_policy", &mut out, 1);
                    continue;
                };
                let (Some(object), Some(spec)) = (
                    object_of(&prov.object),
                    config.sources.get(&m.source_system),
                ) else {
                    bump("unknown_object_or_source", &mut out, 1);
                    continue;
                };
                let payload = row.payload.clone().unwrap_or(Value::Null);
                let wanted: BTreeSet<&String> = policy
                    .writeback_allowlist
                    .iter()
                    .chain(policy.field_mapping.values())
                    .collect();
                let fields: BTreeMap<String, Value> = wanted
                    .into_iter()
                    .filter_map(|f| payload.get(f).map(|v| (f.clone(), v.clone())))
                    .filter(|(_, v)| !v.is_object() && !v.is_array())
                    .collect();
                let modstamp = payload
                    .get(&spec.updated_at_column)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                groups
                    .entry((
                        m.entity_id.clone(),
                        prov.connection_id.clone(),
                        prov.object.clone(),
                    ))
                    .or_default()
                    .push(MemberSnapshot {
                        provider: ProviderScope {
                            org_id: org_id.into(),
                            dataset_id: dataset_id.into(),
                            connection_id: prov.connection_id.clone(),
                            object,
                        },
                        record_key: m.record_key.clone(),
                        external_id: m.source_record_id.clone(),
                        system_modstamp: modstamp,
                        fields,
                    });
            }
            let singletons = groups.values().filter(|g| g.len() < 2).count() as u64;
            bump("singleton_group", &mut out, singletons);
            groups.retain(|_, g| g.len() >= 2);
            if !groups.is_empty() {
                // Elect per (entity, group) with each policy's election.
                let mut by_election: BTreeMap<&str, Vec<(&GroupKey, &MemberSnapshot)>> =
                    BTreeMap::new();
                for (key, members) in &groups {
                    let policy = policies[&(key.1.clone(), key.2.clone())];
                    for member in members {
                        by_election
                            .entry(policy.election_policy.as_str())
                            .or_default()
                            .push((key, member));
                    }
                }
                if protect.is_none() {
                    protect = Some(crate::read::active_protect_overlay(session)?);
                }
                let ranks: BTreeMap<String, i64> = config
                    .sources
                    .iter()
                    .map(|(n, s)| (n.clone(), s.priority_rank))
                    .collect();
                let mut masters: BTreeMap<(String, String), String> = BTreeMap::new();
                for (election, rows) in &by_election {
                    let sql = er_golden::scoped_masters_sql(
                        election,
                        "lake.main",
                        Some(&ranks),
                        protect.unwrap_or(false),
                    )
                    .map_err(|e| e.to_string())?;
                    let entity_ids: Vec<&str> = rows.iter().map(|(k, _)| k.0.as_str()).collect();
                    let group_keys: Vec<String> = rows
                        .iter()
                        .map(|(k, _)| format!("{}|{}", k.1, k.2))
                        .collect();
                    let record_keys: Vec<&str> =
                        rows.iter().map(|(_, m)| m.record_key.as_str()).collect();
                    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
                    let mut result = stmt
                        .query(duckdb::params![
                            entity_ids.join(SEP),
                            group_keys.join(SEP),
                            record_keys.join(SEP)
                        ])
                        .map_err(|e| e.to_string())?;
                    while let Some(r) = result.next().map_err(|e| e.to_string())? {
                        let entity: String = r.get(0).map_err(|e| e.to_string())?;
                        let group: String = r.get(1).map_err(|e| e.to_string())?;
                        let master: String = r.get(2).map_err(|e| e.to_string())?;
                        masters.insert((entity, group), master);
                    }
                }
                let goldens: BTreeMap<String, BTreeMap<String, Value>> =
                    er_entities::golden_record::read::by_ids(
                        conn,
                        snapshot,
                        &groups
                            .keys()
                            .map(|k| k.0.clone())
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .collect::<Vec<_>>()
                            .join(SEP),
                    )
                    .map_err(|e| e.to_string())?
                    .iter()
                    .map(|g| (g.id.clone(), golden_values(g)))
                    .collect();
                // Score evidence: active edges among the groups' members. A
                // lake without a match run has no `match_scores` table, and
                // then no group has evidence.
                let mut edges: BTreeMap<String, Vec<(String, f64)>> = BTreeMap::new();
                if session
                    .lake_table_exists("match_scores")
                    .map_err(|e| e.to_string())?
                {
                    let keys: Vec<&str> = groups
                        .values()
                        .flatten()
                        .map(|m| m.record_key.as_str())
                        .collect();
                    for e in
                        er_entities::match_score::read::among_keys(conn, snapshot, &keys.join(SEP))
                            .map_err(|e| e.to_string())?
                    {
                        edges
                            .entry(e.rec_a_key.clone())
                            .or_default()
                            .push((e.rec_b_key.clone(), e.match_probability));
                        edges
                            .entry(e.rec_b_key)
                            .or_default()
                            .push((e.rec_a_key, e.match_probability));
                    }
                }
                for (key, members) in groups {
                    let policy = policies[&(key.1.clone(), key.2.clone())];
                    let Some(master) =
                        masters.get(&(key.0.clone(), format!("{}|{}", key.1, key.2)))
                    else {
                        bump("no_master", &mut out, 1);
                        continue;
                    };
                    // A golden NULL means "no survivor": no field update. The
                    // materializer never proposes an explicit clear.
                    let golden: BTreeMap<String, Value> = goldens
                        .get(&key.0)
                        .map(|g| {
                            g.iter()
                                .filter(|(_, v)| !v.is_null())
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let input = ProposalInput {
                        scope: ProposalScope {
                            provider: members[0].provider.clone(),
                            entity_id: key.0.clone(),
                            lake_snapshot: snapshot,
                            policy_revision: policy.id.clone(),
                            election_policy: policy.election_policy.clone(),
                        },
                        master_record_key: master.clone(),
                        members,
                        golden_fields: golden,
                        field_mapping: policy
                            .field_mapping
                            .iter()
                            .filter(|(attr, _)| GOLDEN_COLUMNS.contains(&attr.as_str()))
                            .map(|(a, f)| (a.clone(), f.clone()))
                            .collect(),
                        writeback_allowlist: policy.writeback_allowlist.iter().cloned().collect(),
                    };
                    match MergeProposal::build(input.clone()) {
                        Ok(proposal) => out.previews.push(BuiltPreview {
                            score: group_score(&input.members, &input.master_record_key, &edges),
                            origin_run_id: run.id.clone(),
                            policy_id: policy.id.clone(),
                            fingerprint: proposal.fingerprint().to_string(),
                            persisted: serde_json::to_value(PersistedProposalV1::new(input))
                                .map_err(|e| e.to_string())?,
                        }),
                        Err(_) => bump("invalid_proposal", &mut out, 1),
                    }
                }
            }
        }
        out.affected.extend(affected);
        out.next = Some(Checkpoint {
            snapshot_end,
            run_id: run.id.clone(),
        });
        if out.previews.len() >= MAX_GROUPS {
            break;
        }
    }
    out.affected.sort();
    out.affected.dedup();
    Ok(out)
}

/// A group's match evidence: for each victim, its strongest scored edge to
/// another member of the SAME group; then the weakest of those. `None` when
/// any victim has no such edge — a group held together only transitively,
/// or scored by no model, has no evidence to approve on.
fn group_score(
    members: &[MemberSnapshot],
    master: &str,
    edges: &BTreeMap<String, Vec<(String, f64)>>,
) -> Option<f64> {
    let keys: BTreeSet<&str> = members.iter().map(|m| m.record_key.as_str()).collect();
    let mut weakest: Option<f64> = None;
    for victim in members.iter().filter(|m| m.record_key != master) {
        let best = edges
            .get(&victim.record_key)?
            .iter()
            .filter(|(other, _)| keys.contains(other.as_str()))
            .map(|(_, p)| *p)
            .fold(None, |acc: Option<f64>, p| {
                Some(acc.map_or(p, |a| a.max(p)))
            })?;
        weakest = Some(weakest.map_or(best, |w| w.min(best)));
    }
    weakest
}

#[cfg(test)]
mod score_tests {
    use super::*;

    fn member(key: &str) -> MemberSnapshot {
        MemberSnapshot {
            provider: ProviderScope {
                org_id: "o".into(),
                dataset_id: "d".into(),
                connection_id: "c".into(),
                object: crate::writeback::MergeObject::Contact,
            },
            record_key: key.into(),
            external_id: key.into(),
            system_modstamp: String::new(),
            fields: BTreeMap::new(),
        }
    }

    #[test]
    fn the_score_is_the_weakest_victims_strongest_edge_and_absent_evidence_is_none() {
        let members = vec![member("m"), member("v1"), member("v2")];
        let mut edges: BTreeMap<String, Vec<(String, f64)>> = BTreeMap::new();
        for (a, b, p) in [
            ("m", "v1", 0.99),
            ("v1", "v2", 0.80),
            ("m", "v2", 0.70),
            ("v2", "outsider", 1.0),
        ] {
            edges.entry(a.into()).or_default().push((b.into(), p));
            edges.entry(b.into()).or_default().push((a.into(), p));
        }
        // v1's best is 0.99, v2's best inside the group is 0.80 (the 1.0 edge leaves it).
        assert_eq!(group_score(&members, "m", &edges), Some(0.80));
        edges.remove("v2");
        for list in edges.values_mut() {
            list.retain(|(o, _)| o != "v2");
        }
        assert_eq!(
            group_score(&members, "m", &edges),
            None,
            "a victim without evidence"
        );
    }
}
