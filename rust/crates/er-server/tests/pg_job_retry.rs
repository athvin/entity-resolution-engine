//! Job failure classification and retry, over real Postgres (behind
//! `ER_TEST_PG_DSN`).
//!
//! The `attempt` and `max_attempts` columns shipped, and nothing ever retried:
//! a job that failed stayed failed until a human hit resume. Worse, the UI was
//! already built for a world where this worked — `job-detail.tsx` renders an
//! error panel keyed on `error_class` and promises that "transient failures
//! retry automatically", the dashboard counts `precondition` failures, and the
//! runs and queue tables each have a column for it. None of it could ever
//! display anything, because `jobs` had no `error_class` column at all. A
//! failed job showed no reason whatsoever.
//!
//! So these assertions are about two things being true together: the right
//! failures retry, and a steward can see why any of them failed.
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_job_retry

use er_server::pg::{self, FailureClass};

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
        // Transient: worth another go.
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

    // Only transient failures retry. Retrying a precondition or a bad config
    // burns attempts on something that cannot change without a human — and the
    // UI's promise is specifically about transient ones.
    assert!(FailureClass::TransientIo.retryable());
    assert!(!FailureClass::Precondition.retryable());
    assert!(!FailureClass::Config.retryable());
    assert!(!FailureClass::Internal.retryable());

    // The strings are the ones the frontend keys on.
    assert_eq!(FailureClass::Precondition.as_str(), "precondition");
    assert_eq!(FailureClass::TransientIo.as_str(), "transient_io");
}

#[tokio::test]
async fn a_transient_failure_retries_with_backoff_and_a_permanent_one_does_not() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres job retry (set ER_TEST_PG_DSN to run)");
        return;
    };
    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS staged_steward_actions, audit_log, schedules, config_versions, api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::ensure_org(&client, "acme", "/tmp/acme.yaml")
        .await
        .unwrap();
    let org_id: String = client
        .query_one("SELECT id FROM orgs WHERE name = 'acme'", &[])
        .await
        .unwrap()
        .get(0);

    // ---- a transient failure: retried, with the attempt counted ----
    let job = pg::enqueue(&client, &org_id, "run_all_full", 0, None, None, None)
        .await
        .unwrap();
    assert_eq!(job.attempt, 0, "a fresh job has made no attempts");
    assert_eq!(job.error_class, None);

    // Claiming counts the attempt as it hands it out — so a worker that dies
    // without reporting has still used one, which is what stops an infinite
    // retry loop.
    let claimed = pg::claim(&client)
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
        after.state, "queued",
        "a transient failure goes back to the queue"
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
        after.not_before.is_some(),
        "a retry must be held behind a backoff deadline"
    );
    assert!(
        after.finished_at.is_none(),
        "a job that will run again has not finished"
    );

    // And the backoff is real: the job is NOT immediately claimable.
    assert!(
        pg::claim(&client).await.unwrap().is_none(),
        "a backed-off job must not be claimable yet — otherwise the retry is a \
         hot loop, not a backoff"
    );

    // ---- attempts run out ----
    // Drive it to the cap: max_attempts is 3 and one attempt is spent.
    for expected_attempt in 2..=3 {
        client
            .execute(
                "UPDATE jobs SET not_before = now() - interval '1 hour' WHERE id = $1",
                &[&job.id],
            )
            .await
            .unwrap();
        let claimed = pg::claim(&client)
            .await
            .unwrap()
            .expect("the job is claimable once its backoff has passed");
        assert_eq!(claimed.attempt, expected_attempt);
        let after = pg::fail_or_retry(
            &client,
            &job.id,
            FailureClass::TransientIo,
            "connection reset by peer",
        )
        .await
        .unwrap();
        if expected_attempt < 3 {
            assert_eq!(after.state, "queued", "attempts remain");
        } else {
            assert_eq!(
                after.state, "failed",
                "the last attempt fails terminally rather than retrying forever"
            );
            assert!(
                after.finished_at.is_some(),
                "a terminal failure is finished"
            );
        }
    }

    // ---- a permanent failure: not retried, even on the first attempt ----
    let job2 = pg::enqueue(&client, &org_id, "correct", 0, None, None, None)
        .await
        .unwrap();
    client
        .execute(
            "UPDATE jobs SET not_before = NULL WHERE id = $1",
            &[&job2.id],
        )
        .await
        .unwrap();
    let claimed = pg::claim(&client)
        .await
        .unwrap()
        .expect("job2 is claimable");
    assert_eq!(claimed.id, job2.id);
    let after = pg::fail_or_retry(
        &client,
        &job2.id,
        FailureClass::Precondition,
        "no prior run to correct; run a full run first",
    )
    .await
    .unwrap();
    assert_eq!(
        after.state, "failed",
        "a precondition failure must not retry: the missing input appears when a \
         human provides it, not 5 seconds later"
    );
    assert_eq!(after.error_class.as_deref(), Some("precondition"));
    assert_eq!(
        after.attempt, 1,
        "and it did not burn further attempts getting there"
    );

    // ---- a success clears a stale error ----
    // A job that fails, retries and then succeeds must not still show the
    // failure, because the UI renders error_class whenever it is present.
    let job3 = pg::enqueue(&client, &org_id, "lake_maintain", 0, None, None, None)
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
