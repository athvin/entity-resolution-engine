//! The S4.5.3 INV-PERM offer/acceptance fixpoint (port of the PURE core of
//! `src/er/entities/reconcile.py::reconcile_plan`).
//!
//! Given the prior membership partition `p_old` (`entity_id -> members`) and the
//! clustering output `p_new` (groups of `record_key`), decide what a reconcile
//! would write: the changed `entity_membership` assignments, the `entities`
//! transitions, and the `entity_events` (with S4.5.4 `details_hash`). PURE — the
//! only nondeterminism is the ids the injected [`IdFactory`] hands out, minted
//! once per new group in ascending min-member `record_key` order (S4.5.4 D1/D2).

use std::collections::{BTreeMap, BTreeSet};

use er_ids::IdFactory;
use sha2::{Digest, Sha256};

pub const ACTIVE: &str = "active";
pub const MERGED: &str = "merged";
pub const RETIRED: &str = "retired";
pub const CREATED: &str = "created";
pub const MEMBER_ADDED: &str = "member_added";
pub const MEMBER_REMOVED: &str = "member_removed";
pub const SPLIT: &str = "split";
/// `member_removed.cause` for a member the new partition placed elsewhere (S4.5.5).
pub const RECLUSTER: &str = "recluster";
/// `member_removed.cause` for a member that LEFT THE CORPUS — a tombstoned
/// record, which standardize drops.
///
/// Distinct from [`RECLUSTER`] because the two are different facts to anyone
/// reading the ledger: a recluster says the model changed its mind about where
/// a record belongs, a departure says the record is gone. Reporting a deletion
/// as a recluster was this ledger's quietest lie — the only `member_removed`
/// cause that existed, so a departure necessarily arrived mislabelled.
///
/// The CALLER attributes this, not [`reconcile_plan`]: the plan sees only the
/// old and new partitions, and "absent from the new partition" is equally true
/// of a tombstoned record and a suppressed one. Only the stage holding the
/// corpus can tell those apart.
pub const DEPARTED: &str = "departed";

/// S5 event ordering; `entity_events` is emitted in this order then by entity_id
/// then details_hash (S4.5.4 D1).
const PLANNED_EVENT_ORDER: [&str; 6] = [
    CREATED,
    MEMBER_ADDED,
    MEMBER_REMOVED,
    MERGED,
    SPLIT,
    RETIRED,
];

fn event_rank(event_type: &str) -> usize {
    PLANNED_EVENT_ORDER
        .iter()
        .position(|t| *t == event_type)
        .expect("known event type")
}

/// Put `events` in the S4.5.4 D1 order: event type, then entity id, then
/// details digest.
///
/// Public so a caller that APPENDS events — the lake stage's departures and
/// retirements, which need the corpus this crate does not see — can re-impose
/// the same order instead of leaving its own rows in a tail. The ordering is
/// part of the spec, so there must be exactly one implementation of it.
pub fn sort_planned_events(events: &mut [PlannedEvent]) {
    events.sort_by(|a, b| {
        (event_rank(&a.event_type), &a.entity_id, &a.details_hash).cmp(&(
            event_rank(&b.event_type),
            &b.entity_id,
            &b.details_hash,
        ))
    });
}

/// The S4.5.4 details digest: SHA-256 over the canonical JSON (sorted keys,
/// compact, UTF-8) — the same encoding `entity_events.details` stores, so the
/// stored and hashed bytes are one. `details` must already be normalised (its
/// `member_keys` sorted, pairs canonical).
#[must_use]
pub fn details_hash(details: &serde_json::Value) -> String {
    let encoded = serde_json::to_string(details).expect("details serialise");
    let mut hasher = Sha256::new();
    hasher.update(encoded.as_bytes());
    hex::encode(hasher.finalize())
}

/// One `entity_events` row the plan would emit.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedEvent {
    pub entity_id: String,
    pub event_type: String,
    pub details: serde_json::Value,
    pub details_hash: String,
}

impl PlannedEvent {
    /// One planned event, with its [`details_hash`] computed from `details`.
    ///
    /// Public because the lake stage appends events the pure plan cannot
    /// derive — a departure and the retirement it causes both need the corpus,
    /// which this crate deliberately does not know about. Constructing them
    /// through here is what keeps the digest and the stored bytes one thing.
    #[must_use]
    pub fn new(entity_id: &str, event_type: &str, details: serde_json::Value) -> Self {
        let details_hash = details_hash(&details);
        Self {
            entity_id: entity_id.to_string(),
            event_type: event_type.to_string(),
            details,
            details_hash,
        }
    }
}

/// One `entity_membership` row the plan would write — emitted only for a record
/// whose entity actually changed (S4.5.3 D3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipAssignment {
    pub record_key: String,
    pub entity_id: String,
}

/// One `entities` row the plan would insert/update. `merged_into` is `Some` exactly
/// for `MERGED` (S5); `is_new` marks a freshly minted entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityTransition {
    pub entity_id: String,
    pub status: String,
    pub merged_into: Option<String>,
    pub is_new: bool,
}

/// What a reconcile would write. `is_empty()` is INV-PERM's set-equal case.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReconcilePlan {
    pub assignments: Vec<MembershipAssignment>,
    pub transitions: Vec<EntityTransition>,
    pub events: Vec<PlannedEvent>,
    /// Minted ids, in the order taken from the factory (ascending min-member key).
    pub minted: Vec<String>,
}

impl ReconcilePlan {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.assignments.is_empty() && self.transitions.is_empty() && self.events.is_empty()
    }
}

/// S4.5.3's fragment order as an ASCENDING key: `(-member_count, min member)`, so
/// `min` yields rank 1 (largest fragment, min-record_key tiebreak). Total over the
/// fragments of one entity (disjoint, non-empty → unique minima).
fn fragment_rank(members: &BTreeSet<String>) -> (i64, &str) {
    let min = members.iter().next().expect("non-empty fragment").as_str();
    (
        -(i64::try_from(members.len()).expect("fragment fits i64")),
        min,
    )
}

/// Apply INV-PERM to a partition pair; see the module docs.
#[must_use]
pub fn reconcile_plan(
    p_old: &BTreeMap<String, Vec<String>>,
    p_new: &[Vec<String>],
    id_factory: &dyn IdFactory,
) -> ReconcilePlan {
    // Canonicalise: old as entity_id -> set; new groups keyed by min member.
    let old: BTreeMap<String, BTreeSet<String>> = p_old
        .iter()
        .map(|(e, m)| (e.clone(), m.iter().cloned().collect()))
        .collect();
    let mut groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for group in p_new {
        let set: BTreeSet<String> = group.iter().cloned().collect();
        let key = set.iter().next().expect("non-empty group").clone();
        groups.insert(key, set);
    }

    // Overlap matrix: group key -> entity_id -> (g_new ∩ g_old), non-empty cells.
    let mut matrix: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    for (key, members) in &groups {
        let mut cells: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (entity_id, entity_members) in &old {
            let overlap: BTreeSet<String> = members.intersection(entity_members).cloned().collect();
            if !overlap.is_empty() {
                cells.insert(entity_id.clone(), overlap);
            }
        }
        matrix.insert(key.clone(), cells);
    }

    // Offers: each entity offers its id to the group holding its rank-1 fragment.
    let mut fragments: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    for (key, cells) in &matrix {
        for (entity_id, overlap) in cells {
            fragments
                .entry(entity_id.clone())
                .or_default()
                .insert(key.clone(), overlap.clone());
        }
    }
    let offers: BTreeMap<String, String> = fragments
        .iter()
        .map(|(entity_id, by_group)| {
            let key = by_group
                .iter()
                .min_by(|(_, a), (_, b)| fragment_rank(a).cmp(&fragment_rank(b)))
                .map(|(k, _)| k.clone())
                .expect("entity has at least one fragment");
            (entity_id.clone(), key)
        })
        .collect();

    // Acceptances: each offered-to group keeps the entity with the largest overlap.
    let mut offered: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (entity_id, key) in &offers {
        offered
            .entry(key.clone())
            .or_default()
            .push(entity_id.clone());
    }
    let accepted: BTreeMap<String, String> = offered
        .iter()
        .map(|(key, candidates)| {
            let winner = candidates
                .iter()
                .min_by(|a, b| {
                    fragment_rank(&matrix[key][*a]).cmp(&fragment_rank(&matrix[key][*b]))
                })
                .expect("offered group has candidates")
                .clone();
            (key.clone(), winner)
        })
        .collect();

    let mut entity_of_record: BTreeMap<String, String> = BTreeMap::new();
    for (entity_id, members) in &old {
        for member in members {
            entity_of_record.insert(member.clone(), entity_id.clone());
        }
    }
    let mut clustered: BTreeSet<String> = BTreeSet::new();
    for group in groups.values() {
        clustered.extend(group.iter().cloned());
    }
    // Orphans: prior records the clustering placed nowhere (become singletons).
    // entity_of_record keys iterate sorted, so this is already sorted.
    let orphans: Vec<String> = entity_of_record
        .keys()
        .filter(|m| !clustered.contains(*m))
        .cloned()
        .collect();

    // Mint order: unclaimed groups + orphans, one explicit ascending sort.
    let mut mint_keys: Vec<String> = groups
        .keys()
        .filter(|k| !accepted.contains_key(*k))
        .cloned()
        .collect();
    mint_keys.extend(orphans.iter().cloned());
    mint_keys.sort();
    let minted_ids: BTreeMap<String, String> = mint_keys
        .iter()
        .map(|key| (key.clone(), id_factory.new_id()))
        .collect();

    let mut assignments: Vec<MembershipAssignment> = Vec::new();
    let mut transitions: Vec<EntityTransition> = Vec::new();
    let mut events: Vec<PlannedEvent> = Vec::new();

    for (key, members) in &groups {
        let kept = accepted.get(key);
        let entity_id = match kept {
            Some(entity) => entity.clone(),
            None => minted_ids[key].clone(),
        };
        let members_sorted: Vec<String> = members.iter().cloned().collect();
        for member in members {
            if entity_of_record.get(member) != Some(&entity_id) {
                assignments.push(MembershipAssignment {
                    record_key: member.clone(),
                    entity_id: entity_id.clone(),
                });
            }
        }
        match kept {
            None => {
                transitions.push(EntityTransition {
                    entity_id: entity_id.clone(),
                    status: ACTIVE.to_string(),
                    merged_into: None,
                    is_new: true,
                });
                events.push(PlannedEvent::new(
                    &entity_id,
                    CREATED,
                    serde_json::json!({ "member_keys": members_sorted }),
                ));
            }
            Some(kept_entity) => {
                let joined: Vec<String> = members
                    .iter()
                    .filter(|m| !entity_of_record.contains_key(*m))
                    .cloned()
                    .collect();
                if !joined.is_empty() {
                    events.push(PlannedEvent::new(
                        &entity_id,
                        MEMBER_ADDED,
                        serde_json::json!({ "member_keys": joined }),
                    ));
                }
                let left: Vec<String> = old[kept_entity].difference(members).cloned().collect();
                if !left.is_empty() {
                    events.push(PlannedEvent::new(
                        &entity_id,
                        MEMBER_REMOVED,
                        serde_json::json!({ "member_keys": left, "cause": RECLUSTER }),
                    ));
                }
                if old[kept_entity] != *members {
                    transitions.push(EntityTransition {
                        entity_id: entity_id.clone(),
                        status: ACTIVE.to_string(),
                        merged_into: None,
                        is_new: false,
                    });
                }
            }
        }
        // Split: every overlapping entity whose rank-1 fragment is elsewhere left a
        // departing fragment in this group, named on the entity now holding it.
        for (entity, overlap) in &matrix[key] {
            if offers.get(entity).map(String::as_str) != Some(key.as_str()) {
                let overlap_sorted: Vec<String> = overlap.iter().cloned().collect();
                events.push(PlannedEvent::new(
                    &entity_id,
                    SPLIT,
                    serde_json::json!({ "member_keys": overlap_sorted, "split_from": entity }),
                ));
            }
        }
    }

    for entity_id in old.keys() {
        match offers.get(entity_id) {
            None => {
                let members_sorted: Vec<String> = old[entity_id].iter().cloned().collect();
                events.push(PlannedEvent::new(
                    entity_id,
                    RETIRED,
                    serde_json::json!({ "member_keys": members_sorted }),
                ));
                transitions.push(EntityTransition {
                    entity_id: entity_id.clone(),
                    status: RETIRED.to_string(),
                    merged_into: None,
                    is_new: false,
                });
            }
            Some(offered_to) => {
                let claimant = &accepted[offered_to];
                if claimant != entity_id {
                    let overlap_sorted: Vec<String> =
                        matrix[offered_to][entity_id].iter().cloned().collect();
                    events.push(PlannedEvent::new(
                        entity_id,
                        MERGED,
                        serde_json::json!({ "member_keys": overlap_sorted, "merged_into": claimant }),
                    ));
                    transitions.push(EntityTransition {
                        entity_id: entity_id.clone(),
                        status: MERGED.to_string(),
                        merged_into: Some(claimant.clone()),
                        is_new: false,
                    });
                }
            }
        }
    }

    for orphan in &orphans {
        let entity_id = minted_ids[orphan].clone();
        assignments.push(MembershipAssignment {
            record_key: orphan.clone(),
            entity_id: entity_id.clone(),
        });
        events.push(PlannedEvent::new(
            &entity_id,
            CREATED,
            serde_json::json!({ "member_keys": [orphan] }),
        ));
        transitions.push(EntityTransition {
            entity_id,
            status: ACTIVE.to_string(),
            merged_into: None,
            is_new: true,
        });
    }

    assignments.sort_by(|a, b| a.record_key.cmp(&b.record_key));
    transitions.sort_by(|a, b| a.entity_id.cmp(&b.entity_id));
    sort_planned_events(&mut events);
    let minted: Vec<String> = mint_keys.iter().map(|k| minted_ids[k].clone()).collect();

    ReconcilePlan {
        assignments,
        transitions,
        events,
        minted,
    }
}
