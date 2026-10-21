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
//! handles them. This checks the promise, because nothing else did: the kinds
//! are matched as string literals, so a missing arm is not a type error and the
//! compiler has no opinion about it.
//!
//! Textual by necessity — the arms are `match job.kind.as_str()` literals and
//! two of them are async pre-branches in `dispatch_once` rather than arms of
//! that match. Beside `egress_guard.rs` and `mock_contract.rs`, which make the
//! same kind of structural claim the same way.

const DISPATCHER: &str = include_str!("../src/dispatcher.rs");

/// Kinds enqueued by the product but never by a schedule, so they are not in
/// `SCHEDULABLE_KINDS` and still need an arm.
const UNSCHEDULED_KINDS: [&str; 1] = ["apply_corrections"];

/// Whether `dispatcher.rs` routes `kind` — either as a `match` arm on
/// `job.kind`, or as one of `dispatch_once`'s async pre-branches.
fn is_routed(kind: &str) -> bool {
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
fn every_schedulable_job_kind_has_a_dispatcher_arm() {
    assert!(
        DISPATCHER.len() > 1000,
        "the dispatcher source did not load"
    );
    // The scan must be able to see a kind that IS routed, or a typo in the
    // matcher would make every assertion below vacuously true.
    assert!(
        is_routed("lake_maintain"),
        "the routing scan is broken: it cannot find lake_maintain, which is \
         handled by a plain match arm"
    );
    assert!(
        is_routed("apply_corrections"),
        "the routing scan is broken: it cannot find apply_corrections, which is \
         handled by an async pre-branch"
    );
    assert!(
        !is_routed("not_a_real_kind"),
        "the routing scan is broken: it claims an invented kind is routed"
    );

    let unrouted: Vec<&str> = er_server::SCHEDULABLE_KINDS
        .iter()
        .chain(UNSCHEDULED_KINDS.iter())
        .copied()
        .filter(|k| !is_routed(k))
        .collect();
    assert!(
        unrouted.is_empty(),
        "these job kinds can be enqueued but the dispatcher does not handle \
         them, so every one of them fails at run time: {unrouted:?}"
    );
}
