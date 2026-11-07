//! Every kind the queue can hold has somewhere to go.
//!
//! `train` was in `SCHEDULABLE_KINDS` — so the schedule API accepted it, and
//! every tier-C config publish enqueued one — while the dispatcher had no
//! `"train"` arm. It fell through to `other => Err("dispatcher does not handle
//! job kind '…'")` and failed every single time, which meant a config change
//! that needed retraining produced a failed job and a rebuild against the stale
//! model.
//!
//! A list of kinds the scheduler will enqueue is a PROMISE that something
//! handles them. This checks the promise, because nothing else did.
//!
//! ## One destination now
//!
//! Every kind routes to a **compiled DAG**. The `executor.dagKinds` allowlist
//! that used to decide this is gone (#645), so there is no second destination
//! and "somewhere to go" means a DAG for every kind.
//!
//! The DAG half is **registry-backed**: it asks
//! `JobKind::parse` and `dags::dag_for`, so a missing kind is a `None` rather
//! than a missing line of text. Phase 1 of the design said this file would
//! scan `dag_for`; it did not, and the textual scan it kept is what made the
//! file an obstacle to Phase 4 — deleting the legacy arms would have failed
//! this file's own self-check, which hard-asserted that `lake_maintain` had a
//! `match` arm in `dispatcher.rs`.
//!
//! The legacy half has now stood itself down: `LEGACY_ARM_KINDS` is empty, so
//! `kinds_off_the_allowlist_still_have_a_legacy_arm` returns immediately. That
//! is the exit the file was built to take, not a hole — and the list it reads
//! had in any case gone stale, naming five kinds as legacy while the deploy
//! had been routing all eleven to the executor.
//!
//! What checks the level below this one — that each DAG's every TASK has a
//! body — is `dag_task_coverage.rs`.
//!
//! Beside `egress_guard.rs` and `mock_contract.rs`, which make the same kind of
//! structural claim the same way.

use er_server::dags::{self, JobKind};

const DISPATCHER: &str = include_str!("../src/dispatcher.rs");

/// Kinds enqueued by the product but never by a schedule, so they are not in
/// `SCHEDULABLE_KINDS` and still need a destination.
const UNSCHEDULED_KINDS: [&str; 1] = ["apply_corrections"];

/// Kinds that still reach the legacy in-process arm.
///
/// **Empty**: there is no allowlist left to be absent from, so no kind routes
/// anywhere but the executor. The test that reads this stands down on an empty
/// list and is deleted with the arms themselves.
const LEGACY_ARM_KINDS: [&str; 0] = [];

/// Every kind the product can enqueue, from both lists.
fn enqueueable() -> Vec<&'static str> {
    er_server::SCHEDULABLE_KINDS
        .iter()
        .chain(UNSCHEDULED_KINDS.iter())
        .copied()
        .collect()
}

/// Whether `dispatcher.rs` routes `kind` — either as a `match` arm on
/// `job.kind`, or as one of `dispatch_once`'s async pre-branches.
fn has_legacy_arm(kind: &str) -> bool {
    // `"train" =>` / `"run_all_full" | "run_all_incremental" =>` style arms.
    let as_arm = DISPATCHER
        .lines()
        .filter(|l| l.contains("=>"))
        .any(|l| l.contains(&format!("\"{kind}\"")));
    // `job.kind == "apply_corrections"` style pre-branches.
    let as_branch = DISPATCHER.contains(&format!("job.kind == \"{kind}\""));
    as_arm || as_branch
}

#[test]
fn every_enqueueable_job_kind_compiles_to_a_dag() {
    // The mechanism must be able to say no, or every assertion below is
    // vacuously true. `parse` is the gate the whole test rests on.
    assert!(
        JobKind::parse("not_a_real_kind").is_none(),
        "the registry lookup is broken: it resolved an invented kind"
    );
    assert!(
        JobKind::parse("lake_maintain").is_some(),
        "the registry lookup is broken: it cannot resolve a kind that exists"
    );

    let mut unroutable = Vec::new();
    for kind in enqueueable() {
        match JobKind::parse(kind) {
            // Assembling it is the check: `dag_for` panics on a graph that
            // does not validate (the same call `verify_all` makes at boot), and
            // an empty graph would be a kind with nothing to execute.
            Some(parsed) => {
                let dag = dags::dag_for(parsed);
                assert!(
                    !dag.nodes.is_empty(),
                    "{kind} assembles to an EMPTY dag, so an enqueued one would \
                     settle without doing anything"
                );
            }
            None => unroutable.push(kind),
        }
    }
    assert!(
        unroutable.is_empty(),
        "these job kinds can be enqueued but `JobKind::parse` does not know \
         them, so they compile to no DAG at all: {unroutable:?}"
    );
}

#[test]
fn kinds_off_the_allowlist_still_have_a_legacy_arm() {
    // Phase 4's exit condition: with the legacy arm deleted the list is empty
    // and there is nothing left to assert. Standing down is correct here —
    // the DAG test above is what carries the promise afterwards.
    if LEGACY_ARM_KINDS.is_empty() {
        return;
    }
    assert!(
        DISPATCHER.len() > 1000,
        "the dispatcher source did not load"
    );
    // The scan must see a kind that IS routed, and must not see one that is
    // not. Both self-checks use members of the list under test, so they go
    // away with it rather than outliving it.
    assert!(
        LEGACY_ARM_KINDS.iter().any(|k| has_legacy_arm(k)),
        "the routing scan is broken: it found no legacy arm for ANY of \
         {LEGACY_ARM_KINDS:?}"
    );
    assert!(
        !has_legacy_arm("not_a_real_kind"),
        "the routing scan is broken: it claims an invented kind is routed"
    );

    let unrouted: Vec<&str> = LEGACY_ARM_KINDS
        .iter()
        .copied()
        .filter(|k| !has_legacy_arm(k))
        .collect();
    assert!(
        unrouted.is_empty(),
        "these kinds are NOT on the executor allowlist, so they run on the \
         legacy in-process arm — and it does not handle them, which means \
         every one fails at run time: {unrouted:?}"
    );
}

#[test]
fn the_legacy_list_names_only_kinds_the_product_enqueues() {
    // A stale entry here would keep an arm alive for a kind nothing can
    // enqueue, and would make Phase 4 look larger than it is.
    let strangers: Vec<&str> = LEGACY_ARM_KINDS
        .iter()
        .copied()
        .filter(|k| !enqueueable().contains(k))
        .collect();
    assert!(
        strangers.is_empty(),
        "LEGACY_ARM_KINDS names kinds the product never enqueues: {strangers:?}"
    );
}
