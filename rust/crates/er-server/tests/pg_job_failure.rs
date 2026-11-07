//! Job failure classification and recording, over real Postgres (behind
//! `ER_TEST_PG_DSN`).
//!
//! **This file used to be about job-level retry, and that no longer exists.**
//! `fail_or_retry.sql` decided retry in the same statement that recorded the
//! failure — a retryable class with attempts left went back to `queued` behind
//! an exponential backoff — and that branch was the LEGACY in-process arm's
//! retry. #645 deleted the arm, so the branch lost its last production caller
//! and became a terminal record.
//!
//! Retry is per task ATTEMPT now, with Airflow's budget arithmetic, and
//! `pg_task_instances.rs` Phase C owns it: the exact backoff deadline (base
//! plus deterministic hash jitter), the archive/successor lineage, the M → L
//! class escalation on an OOM, and the exhausted budget that leaves the row
//! live in its terminal state. Those phases are not duplicated here, because
//! the thing they would be asserting against is gone.
//!
//! What is left is the half that was always true, and is the reason this file
//! was written: a steward must be able to see WHY a job failed. The UI was
//! built for that before the data existed — `job-detail.tsx` renders an error
//! panel keyed on `error_class`, the dashboard counts `precondition` failures,
//! and the runs and queue tables each have a column for it — and none of it
//! could display anything, because `jobs` had no `error_class` column at all.
//!
//! It is also the only coverage of `pg::FailureClass::classify`, which survives
//! for one caller: `dispatch_pg`, the injected-runner queue harness. The
//! product's copy of those substring rules is
//! `dags::tasks::engine_error_class`, pinned by its own module tests against
//! `er_errors::ErrorClass`. Two implementations, two tables, deliberately — a
//! shared fixture would pass on the day they stopped agreeing.
//!
//! It was `pg_job_retry.rs` until #645. The name was the last thing in the
//! tree still asserting that a job retries.
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test main pg_job_failure

use er_server::pg::{self, FailureClass};

use crate::common;

#[test]
fn failures_are_classified_from_the_messages_this_codebase_produces() {
    // No database needed: this is the taxonomy, and it is matched against the
    // real strings the dispatcher emits rather than a generic guess.
    let cases = [
        // Preconditions: an input is missing. Expected during onboarding.
        (
            "org env missing ER_LAKE_CATALOG",
            FailureClass::Precondition,
        ),
        (
            "no source deliveries found under /app/drop",
            FailureClass::Precondition,
        ),
        (
            "no prior run to correct; run a full run first",
            FailureClass::Precondition,
        ),
        (
            "no model yet (ER_MODEL_PATH); run a full run first",
            FailureClass::Precondition,
        ),
        (
            "org has no drop_root for a run_all",
            FailureClass::Precondition,
        ),
        // The config loader's own prefix.
        (
            "config: thresholds.auto_merge must be > review_low",
            FailureClass::Config,
        ),
        (
            "read config /etc/er/config.yaml: No such file",
            FailureClass::Config,
        ),
        // Transient: a lake open, an object-store read, a connection.
        (
            "open lake: IO Error: Could not open database",
            FailureClass::TransientIo,
        ),
        (
            "read model: Resource temporarily unavailable",
            FailureClass::TransientIo,
        ),
        ("connection reset by peer", FailureClass::TransientIo),
        ("operation timed out", FailureClass::TransientIo),
        // A request that named something invalid is a precondition: what is
        // missing arrives when a CALLER changes the request. These used to fall
        // through to `Internal`, which means "a bug until proven otherwise" and
        // is alarmed on rather than reframed — so a caller's typo looked like
        // an engine defect.
        (
            "a raw profile needs a \"source\" param: two sources' delivered payloads share no \
             schema",
            FailureClass::Precondition,
        ),
        // Anything unrecognised stays internal.
        (
            "dispatcher task panicked: index out of bounds",
            FailureClass::Internal,
        ),
        ("", FailureClass::Internal),
    ];
    for (message, want) in cases {
        assert_eq!(
            FailureClass::classify(message),
            want,
            "classifying {message:?}"
        );
    }

    // The strings are the ones the frontend keys on.
    assert_eq!(FailureClass::Precondition.as_str(), "precondition");
    assert_eq!(FailureClass::TransientIo.as_str(), "transient_io");
    assert_eq!(FailureClass::Config.as_str(), "config");
    assert_eq!(FailureClass::Internal.as_str(), "internal");

    // There is deliberately no `retryable()` assert here. The method went with
    // the requeue branch that was its only caller: this type now describes what
    // a steward is being TOLD, and the retry verdict belongs to
    // `er_errors::ErrorClass` at the executor boundary. An assert that
    // `TransientIo` is "the retryable one" would be decoration — nothing reads
    // it, so nothing could break it.
}

#[tokio::test]
async fn a_job_failure_is_terminal_whatever_its_class_and_carries_its_reason() {
    let Some(db) = common::fresh_db("job_retry").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::ensure_org(&client, "acme", "/tmp/acme.yaml")
        .await
        .unwrap();
    let org_id: String = client
        .query_one("SELECT id FROM orgs WHERE name = 'acme'", &[])
        .await
        .unwrap()
        .get(0);

    // ---- a transient failure: the class that USED to retry ----------------
    let job = pg::enqueue(
        &client,
        &org_id,
        None,
        "run_all_full",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    assert_eq!(job.attempt, 0, "a fresh job has made no attempts");
    assert_eq!(job.error_class, None);

    // Claiming still counts the attempt as it hands it out — that is what
    // `resume` then spends, and it is the one part of the attempt bookkeeping
    // this change did not touch.
    let claimed = pg::claim(&client, 1)
        .await
        .unwrap()
        .expect("the job is claimable");
    assert_eq!(claimed.id, job.id);
    assert_eq!(claimed.attempt, 1, "claiming counts the attempt");

    let after = pg::fail_or_retry(
        &client,
        &job.id,
        FailureClass::TransientIo,
        "open lake: IO Error: Could not open database",
    )
    .await
    .unwrap();
    assert_eq!(
        after.state, "failed",
        "a transient failure is TERMINAL now. It went back to `queued` behind a \
         backoff while the legacy in-process arm existed to run the retry; with \
         the arm gone there is nothing to requeue it FOR, and a row cycling \
         through `queued` that no dispatcher will execute is worse than a \
         failure a steward can see and resume"
    );
    assert_eq!(after.error_class.as_deref(), Some("transient_io"));
    assert!(
        after
            .error_detail
            .as_deref()
            .is_some_and(|d| d.contains("Could not open database")),
        "the reason must be stored, or a steward is back to reading pod logs: {:?}",
        after.error_detail
    );
    assert!(
        after.finished_at.is_some(),
        "a terminal failure is finished — the requeue branch cleared this \
         stamp, and nothing clears it now"
    );

    // And it left the QUEUE, not merely the current moment. The old assertion
    // here was that `claim` returns nothing, which held for the retry too — it
    // was behind a backoff deadline. So clear the deadline first: what makes
    // this row unclaimable must be its terminal state and nothing else.
    client
        .execute(
            "UPDATE jobs SET not_before = NULL WHERE id = $1",
            &[&job.id],
        )
        .await
        .unwrap();
    assert!(
        pg::claim(&client, 1).await.unwrap().is_none(),
        "a failed job is out of the queue with its deadline cleared; if this \
         claims, the requeue branch is back"
    );
    let attempt_now: i64 = client
        .query_one("SELECT attempt FROM jobs WHERE id = $1", &[&job.id])
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        attempt_now, 1,
        "and it burned exactly the one attempt it was handed, rather than \
         spending the budget on retries nothing would have run"
    );

    // ---- a permanent failure: recorded identically, class aside -----------
    // The classes used to diverge HERE, which is what the old test was for.
    // They no longer do: the class is provenance for the steward, and the only
    // observable difference between these two rows is the `error_class` string
    // the UI renders.
    let job2 = pg::enqueue(
        &client,
        &org_id,
        None,
        "correct",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    client
        .execute(
            "UPDATE jobs SET not_before = NULL WHERE id = $1",
            &[&job2.id],
        )
        .await
        .unwrap();
    let claimed = pg::claim(&client, 1)
        .await
        .unwrap()
        .expect("job2 is claimable");
    assert_eq!(claimed.id, job2.id);
    let after2 = pg::fail_or_retry(
        &client,
        &job2.id,
        FailureClass::Precondition,
        "no prior run to correct; run a full run first",
    )
    .await
    .unwrap();
    assert_eq!(after2.state, after.state, "same state as the transient one");
    assert_eq!(after2.error_class.as_deref(), Some("precondition"));
    assert_eq!(after2.attempt, 1);
    assert!(after2.finished_at.is_some());

    // `max_attempts` is the column that paid for this. It is SELECTed by every
    // `jobs` statement's RETURNING list and consulted by NONE of them — the
    // requeue branch was its only reader. It stays on the table regardless:
    // dropping a column is a breaking schema change the reconciler refuses, and
    // the budget that matters is `task_instances.max_tries`.
    assert_eq!(after2.max_attempts, 3, "still recorded, read by nothing");

    // ---- a success clears a stale error ----
    // A job that fails, is resumed and then succeeds must not still show the
    // failure, because the UI renders error_class whenever it is present.
    let job3 = pg::enqueue(
        &client,
        &org_id,
        None,
        "lake_maintain",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    let _ = pg::fail_or_retry(
        &client,
        &job3.id,
        FailureClass::TransientIo,
        "operation timed out",
    )
    .await
    .unwrap();
    pg::mark(&client, &job3.id, "succeeded").await.unwrap();
    let row = client
        .query_one(
            "SELECT state, error_class, error_detail FROM jobs WHERE id = $1",
            &[&job3.id],
        )
        .await
        .unwrap();
    let state: String = row.get(0);
    let class: Option<String> = row.get(1);
    let detail: Option<String> = row.get(2);
    assert_eq!(state, "succeeded");
    assert_eq!(
        class, None,
        "a job that finally succeeded must not still report an error"
    );
    assert_eq!(detail, None);
}
