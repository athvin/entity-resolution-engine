//! Incremental (base -> batch) scenario gates (S8.2). Each scenario resolves a
//! `base` phase, then a `batch` phase whose delivery changes the clustering, and
//! both phases' membership partitions + event profiles are checked against the
//! committed goldens (id-insensitively — Rust mints its own entity ids).
//!
//! As the Python scenario tests do, both phases score the full cumulative corpus
//! (`match --mode full`; reconcile auto-upgrades to full), so a batch is
//! `reconcile_plan(p_old = prior membership, p_new = connected components over
//! the cumulative corpus)` folded onto the prior membership. This exercises
//! every reconcile transition end-to-end: merge, member_added (merge_scenario*),
//! split + member_removed + created (split_scenario*), and content supersession
//! (supersession_scenario).

mod common;

use std::collections::{BTreeMap, BTreeSet};

use dedupe_core::session::{Session, SessionOptions};
use er_ids::CountingIdFactory;
use er_reconcile::reconcile::ReconcilePlan;

fn expected(scenario: &str, phase: &str, relation: &str) -> String {
    let path = format!(
        "{}/../../../fixtures/static/{scenario}/expected/{phase}/{relation}.csv",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

/// The set of clusters (each a sorted record-key set) of a full membership.
fn partition_of(membership: &BTreeMap<String, Vec<String>>) -> BTreeSet<BTreeSet<String>> {
    membership
        .values()
        .map(|members| members.iter().cloned().collect())
        .collect()
}

/// Partition from a committed membership.csv (persona_id, source_system,
/// source_record_id, entity_label): group `source:record_id` by entity_label.
fn partition_csv(csv: &str) -> BTreeSet<BTreeSet<String>> {
    let mut by_label: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for line in csv.lines().skip(1).filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split(',').collect();
        by_label
            .entry(f[3].to_string())
            .or_default()
            .insert(format!("{}:{}", f[1], f[2]));
    }
    by_label.into_values().collect()
}

/// The sorted multiset of per-entity `{event_type: count}` profiles.
fn event_profiles(
    per_entity: BTreeMap<String, BTreeMap<String, usize>>,
) -> Vec<Vec<(String, usize)>> {
    let mut out: Vec<Vec<(String, usize)>> = per_entity
        .into_values()
        .map(|m| m.into_iter().collect())
        .collect();
    out.sort();
    out
}

fn events_of(plan: &ReconcilePlan) -> Vec<Vec<(String, usize)>> {
    let mut by_entity: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for e in &plan.events {
        *by_entity
            .entry(e.entity_id.clone())
            .or_default()
            .entry(e.event_type.clone())
            .or_default() += 1;
    }
    event_profiles(by_entity)
}

fn events_csv(csv: &str) -> Vec<Vec<(String, usize)>> {
    let mut by_label: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for line in csv.lines().skip(1).filter(|l| !l.is_empty()) {
        // entity_label, event_type, count
        let f: Vec<&str> = line.split(',').collect();
        by_label
            .entry(f[0].to_string())
            .or_default()
            .insert(f[1].to_string(), f[2].parse().unwrap());
    }
    event_profiles(by_label)
}

/// Resolve `base` then `batch`, asserting both phases' membership + events
/// against the committed goldens.
fn run_two_phase(scenario: &str) {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let config = common::scenario_init(&session);
    // One id source threaded across phases, so a batch's new entities never
    // collide with ids minted in the base phase.
    let ids = CountingIdFactory::new(0);

    // base phase: empty prior.
    common::land_phase(
        &session,
        &config,
        scenario,
        "base",
        &format!("{scenario}-base"),
        "2026-01-01 00:00:00",
    );
    common::rebuild_std(&session, &config);
    let base_never = common::load_never_pairs(scenario, "base");
    let base_plan =
        common::resolve_with_prior(&session, &config, &BTreeMap::new(), &ids, &base_never);
    let base_mem = common::fold_plan(&BTreeMap::new(), &base_plan);
    assert_eq!(
        partition_of(&base_mem),
        partition_csv(&expected(scenario, "base", "membership")),
        "{scenario}: base membership differs from golden"
    );
    assert_eq!(
        events_of(&base_plan),
        events_csv(&expected(scenario, "base", "events")),
        "{scenario}: base events differ from golden"
    );

    // batch phase: prior = base membership; the delivery reshapes the clusters.
    common::land_phase(
        &session,
        &config,
        scenario,
        "batch",
        &format!("{scenario}-batch"),
        "2026-06-01 12:00:00",
    );
    common::rebuild_std(&session, &config);
    let batch_never = common::load_never_pairs(scenario, "batch");
    let batch_plan = common::resolve_with_prior(&session, &config, &base_mem, &ids, &batch_never);
    let batch_mem = common::fold_plan(&base_mem, &batch_plan);
    assert_eq!(
        partition_of(&batch_mem),
        partition_csv(&expected(scenario, "batch", "membership")),
        "{scenario}: batch membership differs from golden"
    );
    assert_eq!(
        events_of(&batch_plan),
        events_csv(&expected(scenario, "batch", "events")),
        "{scenario}: batch events differ from golden"
    );
}

#[test]
fn merge_scenario() {
    run_two_phase("merge_scenario");
}

#[test]
fn merge_scenario_tie() {
    run_two_phase("merge_scenario_tie");
}

#[test]
fn split_scenario() {
    run_two_phase("split_scenario");
}

#[test]
fn split_scenario_tie_2_2() {
    run_two_phase("split_scenario_tie_2_2");
}

#[test]
fn supersession_scenario() {
    run_two_phase("supersession_scenario");
}
