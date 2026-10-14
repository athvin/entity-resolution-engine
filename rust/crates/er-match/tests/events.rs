//! The entity-events gate: the reconcile plan's `events` reproduce base_10's
//! committed `events.csv` (one `created` per entity, id-insensitively). Compared
//! as the multiset of per-entity `{event_type: count}` profiles, so Rust's
//! minted ids and the golden's `E*` labels drop out.

mod common;

use std::collections::BTreeMap;

use dedupe_core::session::{Session, SessionOptions};

/// The sorted multiset of per-entity event-type-count profiles.
fn profiles(per_entity: BTreeMap<String, BTreeMap<String, usize>>) -> Vec<Vec<(String, usize)>> {
    let mut out: Vec<Vec<(String, usize)>> = per_entity
        .into_values()
        .map(|m| m.into_iter().collect())
        .collect();
    out.sort();
    out
}

#[test]
fn base_10_entity_events_match_committed() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let (_config, plan) = common::resolve_base_10(&session);

    let mut got: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for event in &plan.events {
        *got.entry(event.entity_id.clone())
            .or_default()
            .entry(event.event_type.clone())
            .or_default() += 1;
    }

    let mut want: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for line in include_str!("fixtures/events_base_10.csv")
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
    {
        // entity_label, event_type, count
        let f: Vec<&str> = line.split(',').collect();
        want.entry(f[0].to_string())
            .or_default()
            .insert(f[1].to_string(), f[2].parse().unwrap());
    }

    let got = profiles(got);
    assert_eq!(got.len(), 10, "base_10 base mints 10 entities");
    assert_eq!(
        got,
        profiles(want),
        "entity_events profiles differ from the committed golden"
    );
}
