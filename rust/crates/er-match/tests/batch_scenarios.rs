//! Incremental (base -> batch) scenario gates (S8.2). Each scenario resolves a
//! `base` phase, then a `batch` phase whose delivery (or steward assertion)
//! reshapes the clustering, checked against the committed goldens
//! (id-insensitively — Rust mints its own entity ids).
//!
//! As the Python scenario tests do, both phases score the full cumulative corpus
//! (`match --mode full`; reconcile auto-upgrades to full), so a batch is
//! `reconcile_plan(p_old = prior membership, p_new = connected components over
//! the cumulative corpus — after never-cut)` folded onto the prior membership.
//! Exercises merge/member_added, split/member_removed/created, content
//! supersession, and `never`-cut severing — both the direct bridge (edge
//! adjustment, no `edge_cut`) and partition-level path cuts (one `edge_cut`
//! event per cut edge, attributed to the severed entity). Membership AND events
//! are gated for all of these.
//!
//! `always`-assertion merges + assertion retraction (assertions_scenario) and
//! tombstone deletion (deletion_scenario) are the remaining assertion chunk.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use dedupe_core::session::{Session, SessionOptions};
use er_ids::CountingIdFactory;
use er_reconcile::assertions::check_contradiction_1;
use er_reconcile::reconcile::ReconcilePlan;

/// An expected relation if the scenario commits one for the phase; `None` means
/// the phase makes no claim about it (S8.2.1).
fn expected_opt(scenario: &str, phase: &str, relation: &str) -> Option<String> {
    let path = format!(
        "{}/../../../fixtures/static/{scenario}/expected/{phase}/{relation}.csv",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).ok()
}

fn expected(scenario: &str, phase: &str, relation: &str) -> String {
    expected_opt(scenario, phase, relation)
        .unwrap_or_else(|| panic!("{scenario}/{phase}/{relation}.csv not committed"))
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

/// Resolve `base` then `batch`. Base membership + events and batch membership are
/// always asserted; batch events only when `gate_batch_events` (a scenario whose
/// batch emits `edge_cut` is membership-only until cut events are produced).
fn run_two_phase(scenario: &str, gate_batch_events: bool) {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let config = common::scenario_init(&session);
    // One id source threaded across phases, so a batch's new entities never
    // collide with ids minted in the base phase.
    let ids = CountingIdFactory::new(0);

    common::land_phase(
        &session,
        &config,
        scenario,
        "base",
        &format!("{scenario}-base"),
        "2026-01-01 00:00:00",
    );
    common::rebuild_std(&session, &config);
    let base_assertions = common::load_assertions(scenario, "base");
    let base_plan =
        common::resolve_with_prior(&session, &config, &BTreeMap::new(), &ids, &base_assertions);
    let base_mem = common::fold_plan(&BTreeMap::new(), &base_plan);
    assert_eq!(
        partition_of(&base_mem),
        partition_csv(&expected(scenario, "base", "membership")),
        "{scenario}: base membership differs from golden"
    );
    if let Some(golden) = expected_opt(scenario, "base", "events") {
        assert_eq!(
            events_of(&base_plan),
            events_csv(&golden),
            "{scenario}: base events differ from golden"
        );
    }

    common::land_phase(
        &session,
        &config,
        scenario,
        "batch",
        &format!("{scenario}-batch"),
        "2026-06-01 12:00:00",
    );
    common::rebuild_std(&session, &config);
    let batch_assertions = common::load_assertions(scenario, "batch");
    let batch_plan =
        common::resolve_with_prior(&session, &config, &base_mem, &ids, &batch_assertions);
    let batch_mem = common::fold_plan(&base_mem, &batch_plan);
    assert_eq!(
        partition_of(&batch_mem),
        partition_csv(&expected(scenario, "batch", "membership")),
        "{scenario}: batch membership differs from golden"
    );
    if gate_batch_events {
        assert_eq!(
            events_of(&batch_plan),
            events_csv(&expected(scenario, "batch", "events")),
            "{scenario}: batch events differ from golden"
        );
    }
}

// Full membership + events parity (no edge_cut in these batches).
#[test]
fn merge_scenario() {
    run_two_phase("merge_scenario", true);
}

#[test]
fn merge_scenario_tie() {
    run_two_phase("merge_scenario_tie", true);
}

#[test]
fn split_scenario() {
    run_two_phase("split_scenario", true);
}

#[test]
fn split_scenario_tie_2_2() {
    run_two_phase("split_scenario_tie_2_2", true);
}

#[test]
fn supersession_scenario() {
    run_two_phase("supersession_scenario", true);
}

// never-cut partition splits — membership + events parity (incl. edge_cut).
#[test]
fn assertions_path_tie() {
    run_two_phase("assertions_path_tie", true);
}

#[test]
fn assertions_cut_tie() {
    run_two_phase("assertions_cut_tie", true);
}

#[test]
fn assertions_two_iterations() {
    run_two_phase("assertions_two_iterations", true);
}

#[test]
fn assertions_non_batch() {
    run_two_phase("assertions_non_batch", true);
}

/// CONTRADICTION-1 (S4.4.1): an `always`-closure that also carries a `never` over
/// the same component is unsatisfiable, and the batch must refuse — the fixture
/// commits no batch output. We assert the check finds the contradiction.
#[test]
fn assertions_contradiction_refuses() {
    let batch_assertions = common::load_assertions("assertions_contradiction", "batch");
    let findings = check_contradiction_1(&batch_assertions);
    assert!(
        !findings.is_empty(),
        "expected CONTRADICTION-1 to reject the batch assertions"
    );
}
