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

/// Phase ingested_at stamps, strictly increasing so supersession/tombstone
/// ordering (latest row wins) follows phase order.
const PHASE_TS: [&str; 3] = [
    "2026-01-01 00:00:00",
    "2026-06-01 12:00:00",
    "2026-09-01 12:00:00",
];

/// Run a scenario's phases in order — each `(name, full_refresh, gate_events)` —
/// threading the prior membership and one id factory across them. Every phase
/// asserts membership; events are asserted when `gate_events` and the phase
/// commits an events golden (S8.2.1 absent = no claim).
fn run_phases(scenario: &str, phases: &[(&str, bool, bool)]) {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let config = common::scenario_init(&session);
    let ids = CountingIdFactory::new(0);
    let mut p_old: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for (i, (phase, full_refresh, gate_events)) in phases.iter().enumerate() {
        common::land_phase(
            &session,
            &config,
            scenario,
            phase,
            &format!("{scenario}-{phase}"),
            PHASE_TS[i],
            *full_refresh,
        );
        common::rebuild_std(&session, &config);
        let assertions = common::phase_assertions(scenario, phase);
        let plan = common::resolve_with_prior(&session, &config, &p_old, &ids, &assertions);
        let mem = common::fold_plan(&p_old, &plan, &common::live_records(&session));

        assert_eq!(
            partition_of(&mem),
            partition_csv(&expected(scenario, phase, "membership")),
            "{scenario}/{phase}: membership differs from golden"
        );
        if *gate_events && let Some(golden) = expected_opt(scenario, phase, "events") {
            assert_eq!(
                events_of(&plan),
                events_csv(&golden),
                "{scenario}/{phase}: events differ from golden"
            );
        }
        p_old = mem;
    }
}

/// The ordinary base->batch scenarios: both phases full membership + events.
const BASE_BATCH: [(&str, bool, bool); 2] = [("base", false, true), ("batch", false, true)];

#[test]
fn merge_scenario() {
    run_phases("merge_scenario", &BASE_BATCH);
}

#[test]
fn merge_scenario_tie() {
    run_phases("merge_scenario_tie", &BASE_BATCH);
}

#[test]
fn split_scenario() {
    run_phases("split_scenario", &BASE_BATCH);
}

#[test]
fn split_scenario_tie_2_2() {
    run_phases("split_scenario_tie_2_2", &BASE_BATCH);
}

#[test]
fn supersession_scenario() {
    run_phases("supersession_scenario", &BASE_BATCH);
}

// never-cut partition splits — membership + events parity (incl. edge_cut).
#[test]
fn assertions_path_tie() {
    run_phases("assertions_path_tie", &BASE_BATCH);
}

#[test]
fn assertions_cut_tie() {
    run_phases("assertions_cut_tie", &BASE_BATCH);
}

#[test]
fn assertions_two_iterations() {
    run_phases("assertions_two_iterations", &BASE_BATCH);
}

#[test]
fn assertions_non_batch() {
    run_phases("assertions_non_batch", &BASE_BATCH);
}

/// Three-phase deletion (S8.2): `base` resolves, `refresh` is a full-refresh
/// delivery tombstoning the two omitted keys (the bridge W321 -> split; the
/// singleton C323 -> retire), and `resurrect` re-delivers W321 with corrected
/// values, merging the fragments back. Membership (all three phases) matches;
/// events are gated only on `base` (whose `created`s come from the pure core).
/// The tombstone-driven `member_removed`/`retired` attribution on `refresh` is a
/// reconcile-STAGE concern (relational_stage.py), not in the pure `reconcile_plan`
/// core this gate drives — deferred with the edge_cut attribution.
#[test]
fn deletion_scenario() {
    run_phases(
        "deletion_scenario",
        &[
            ("base", false, true),
            ("refresh", true, true),
            ("resurrect", false, true),
        ],
    );
}

/// The full assertion scenario: an `always` force-merge (A1), a direct `never`
/// (A2, edge adjustment), a path `never` forcing a partition cut (A3, edge_cut),
/// and a retracted `always` (A4, active=false in the reconciled set) that must
/// have no effect. Membership only: the partition (merge + both splits +
/// retraction) matches, but the `edge_cut` event's entity ATTRIBUTION here
/// differs from the single-prior-entity path scenarios (the golden attaches it
/// to the newly-created split fragment, not the prior entity); nailing that rule
/// down needs the Python reconcile source and is deferred.
#[test]
fn assertions_scenario() {
    run_phases(
        "assertions_scenario",
        &[("base", false, true), ("batch", false, true)],
    );
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
