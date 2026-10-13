//! The S4.5.3 INV-PERM offer/acceptance fixpoint (port of the PURE core of
//! `src/er/entities/reconcile.py::reconcile_plan`).
//!
//! Given the prior membership partition `p_old` (`entity_id -> members`) and the
//! clustering output `p_new` (groups of `record_key`), decide what a reconcile
//! would write: the changed `entity_membership` assignments and the `entities`
//! transitions. PURE — the only nondeterminism is the ids the injected
//! [`IdFactory`] hands out, minted once per new group in ascending min-member
//! `record_key` order (S4.5.4 D1/D2). Entity-level events (`entity_events` +
//! `details_hash`) are a later addition; this returns assignments + transitions.

use std::collections::{BTreeMap, BTreeSet};

use er_ids::IdFactory;

pub const ACTIVE: &str = "active";
pub const MERGED: &str = "merged";
pub const RETIRED: &str = "retired";

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
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconcilePlan {
    pub assignments: Vec<MembershipAssignment>,
    pub transitions: Vec<EntityTransition>,
    /// Minted ids, in the order taken from the factory (ascending min-member key).
    pub minted: Vec<String>,
}

impl ReconcilePlan {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.assignments.is_empty() && self.transitions.is_empty()
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

    for (key, members) in &groups {
        let kept = accepted.get(key);
        let entity_id = match kept {
            Some(entity) => entity.clone(),
            None => minted_ids[key].clone(),
        };
        for member in members {
            if entity_of_record.get(member) != Some(&entity_id) {
                assignments.push(MembershipAssignment {
                    record_key: member.clone(),
                    entity_id: entity_id.clone(),
                });
            }
        }
        match kept {
            None => transitions.push(EntityTransition {
                entity_id,
                status: ACTIVE.to_string(),
                merged_into: None,
                is_new: true,
            }),
            Some(kept_entity) => {
                if old[kept_entity] != *members {
                    transitions.push(EntityTransition {
                        entity_id,
                        status: ACTIVE.to_string(),
                        merged_into: None,
                        is_new: false,
                    });
                }
            }
        }
    }

    for entity_id in old.keys() {
        match offers.get(entity_id) {
            None => transitions.push(EntityTransition {
                entity_id: entity_id.clone(),
                status: RETIRED.to_string(),
                merged_into: None,
                is_new: false,
            }),
            Some(offered_to) => {
                let claimant = &accepted[offered_to];
                if claimant != entity_id {
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
        transitions.push(EntityTransition {
            entity_id,
            status: ACTIVE.to_string(),
            merged_into: None,
            is_new: true,
        });
    }

    assignments.sort_by(|a, b| a.record_key.cmp(&b.record_key));
    transitions.sort_by(|a, b| a.entity_id.cmp(&b.entity_id));
    let minted: Vec<String> = mint_keys.iter().map(|k| minted_ids[k].clone()).collect();

    ReconcilePlan {
        assignments,
        transitions,
        minted,
    }
}
