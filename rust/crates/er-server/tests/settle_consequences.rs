//! Everything a settled job is supposed to produce, the executor produces.
//!
//! A job settling has consequences beyond its row: the `job.<state>` webhook,
//! the `entity.*` aggregate, and the run-report email. The legacy in-process
//! arm did all three at the end of `dispatch_claimed`. The executor is the only
//! path a job takes on a cluster, and it did two.
//!
//! `run_report_messages` had exactly one caller — the legacy arm — while
//! `deploy.yml` routed every kind to the executor. So the run report had
//! already stopped being sent in dev and in production, and nothing reported
//! it, because an email that is not sent looks exactly like a tenant who never
//! configured one. Deleting the arm (#645) around it would have made that
//! permanent.
//!
//! The lesson generalises past this one function: a settle consequence that
//! lives only on the path being deleted is invisible until someone misses the
//! mail. So each one is named here, with the CALL SITE that has to exist.
//!
//! Call site, not mention. The first version of this gate asked only whether
//! `executor.rs` contained each name, and it passed when the call to
//! `send_run_report` was deleted out of the settle — because the method, and
//! the `run_report_messages` inside it, were still sitting there unreached.
//! It would have caught the original bug (the name was absent entirely) and
//! not the next one, which is the kind of weakness that reads as a clean tree.
//!
//! This is a claim about REACHABILITY, which is the half that broke. The
//! behaviour of each consequence is covered where it belongs:
//! `run_report_messages`'s rules are unit-tested in `dispatcher.rs` without a
//! mailer, and the webhook and the aggregate have their own Postgres-tier
//! tests.
//!
//! Beside `dag_task_coverage.rs`, which makes the same kind of reachability
//! claim one level down, about task bodies rather than settle consequences.

const EXECUTOR: &str = include_str!("../src/executor.rs");

/// Each settle consequence: the call expression that must appear in
/// `executor.rs`, and where the consequence's own behaviour is proven.
///
/// A consequence reached through a private helper needs the helper's CALL
/// (`self.…(`), not its definition — a definition proves only that the code
/// exists, which is what the unreached `send_run_report` already proved.
const CONSEQUENCES: [(&str, &str); 3] = [
    (
        "emit_job_event(",
        "one job.<state> webhook per outcome (pg_webhook_deliver.rs)",
    ),
    (
        "emit_entity_aggregates(",
        "at most one entity.* per kind (pg_entity_events.rs)",
    ),
    (
        "self.send_run_report(",
        "the run report, one message per recipient (dispatcher.rs unit tests)",
    ),
];

#[test]
fn the_executor_settle_produces_every_consequence() {
    // The scan must be able to say no, or the assertion below is vacuous.
    assert!(
        !EXECUTOR.contains("self.emit_a_consequence_that_does_not_exist("),
        "the source scan is broken: it found an invented call"
    );
    assert!(
        EXECUTOR.len() > 1000,
        "executor.rs did not load — the include_str! path is wrong"
    );

    let missing: Vec<&str> = CONSEQUENCES
        .iter()
        .filter(|(call, _)| !EXECUTOR.contains(call))
        .map(|(call, _)| *call)
        .collect();
    assert!(
        missing.is_empty(),
        "the executor is the only path a job settles on, so every settle \
         consequence has to be CALLED from it. Not called in executor.rs: \
         {missing:?}.\nEach is listed in this file with where its behaviour is \
         proven; a consequence left unreached is silently lost, and an email \
         that is never sent looks exactly like a tenant who configured none."
    );

    // The helper has to reach the pure decision, or the call above lands in a
    // body that does nothing.
    assert!(
        EXECUTOR.contains("run_report_messages("),
        "send_run_report is called but never reaches run_report_messages"
    );
}
