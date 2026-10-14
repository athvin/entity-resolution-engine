//! The end-to-end entity-membership gate: standardize -> match -> cluster ->
//! reconcile, and assert the resulting partition equals base_10's committed
//! `membership.csv` (ID-insensitively — Rust mints its own entity ids).
//!
//! This is T-INV-1 in miniature ("membership equals connected components"):
//! records are auto-merged (`match_probability >= auto_merge`) into connected
//! components and `reconcile_plan` (empty prior) mints one entity per component.
//! The discriminating trap is crm:C006 — auto-merged to billing:B005 (E5) but
//! NOT to webforms:W006, because crm:C006-webforms:W006 is the single gray-band
//! pair (0.857 < auto_merge), so the shared household stays two entities.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use dedupe_core::session::{Session, SessionOptions};

/// The partition as a set of clusters, each a sorted set of record keys. Entity
/// labels are dropped, so Rust's minted ids and the golden's `E*` labels compare
/// equal whenever they induce the same grouping.
fn partition<I: IntoIterator<Item = (String, String)>>(pairs: I) -> BTreeSet<BTreeSet<String>> {
    let mut by_entity: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (entity, record_key) in pairs {
        by_entity.entry(entity).or_default().insert(record_key);
    }
    by_entity.into_values().collect()
}

#[test]
fn base_10_membership_matches_committed_partition() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let (_config, plan) = common::resolve_base_10(&session);

    let got = partition(
        plan.assignments
            .iter()
            .map(|a| (a.entity_id.clone(), a.record_key.clone())),
    );

    // Expected partition from the committed golden: group record_key by entity_label.
    let want = partition(
        include_str!("fixtures/membership_base_10.csv")
            .lines()
            .skip(1)
            .filter(|l| !l.is_empty())
            .map(|line| {
                let f: Vec<&str> = line.split(',').collect();
                // persona_id, source_system, source_record_id, entity_label
                (f[3].to_string(), format!("{}:{}", f[1], f[2]))
            }),
    );

    assert_eq!(
        got.len(),
        10,
        "base_10 resolves to 10 entities; got {}",
        got.len()
    );
    assert_eq!(
        got, want,
        "membership partition differs from the committed golden"
    );
}
