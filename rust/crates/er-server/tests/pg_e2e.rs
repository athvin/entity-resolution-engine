//! Postgres job-queue gate (behind `ER_TEST_PG_DSN`): exercises the real
//! SKIP-LOCKED claim, the one-active-per-org constraint, and idempotent enqueue
//! against a live Postgres. Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_e2e

use er_server::pg;

#[tokio::test]
async fn queue_lifecycle_over_postgres() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres queue e2e (set ER_TEST_PG_DSN to run)");
        return;
    };
    let mut client = pg::connect(&dsn).await.unwrap();

    // Clean slate (api_keys + jobs both reference orgs), then apply the schema.
    client
        .batch_execute(
            "DROP TABLE IF EXISTS api_keys; DROP TABLE IF EXISTS jobs; DROP TABLE IF EXISTS orgs;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::ensure_org(&client, "acme", "/configs/acme.yaml")
        .await
        .unwrap();

    // Enqueue two jobs; claim takes the higher priority first.
    let high = pg::enqueue(&client, "acme", "run", 5, None).await.unwrap();
    let _low = pg::enqueue(&client, "acme", "run", 0, None).await.unwrap();
    assert_eq!(high.state, "queued");

    let claimed = pg::claim(&client).await.unwrap().expect("a job to claim");
    assert_eq!(claimed.job_id, high.job_id);
    assert_eq!(claimed.state, "dispatching");

    // One active job per org: the second claim finds nothing while `high` runs.
    assert!(
        pg::claim(&client).await.unwrap().is_none(),
        "one-active-per-org must block the second claim"
    );

    // Finish the first; now the low-priority job is claimable.
    pg::mark(&client, &high.job_id, "succeeded").await.unwrap();
    let next = pg::claim(&client)
        .await
        .unwrap()
        .expect("low job now claimable");
    assert_eq!(next.priority, 0);
    pg::mark(&client, &next.job_id, "succeeded").await.unwrap();

    // Idempotent enqueue: the same key returns the original job.
    let a = pg::enqueue(&client, "acme", "run", 0, Some("k1"))
        .await
        .unwrap();
    let b = pg::enqueue(&client, "acme", "run", 0, Some("k1"))
        .await
        .unwrap();
    assert_eq!(a.job_id, b.job_id, "idempotency key must dedupe");

    // list_jobs sees all four distinct jobs (high, low, and the one idempotent).
    let jobs = pg::list_jobs(&client, "acme").await.unwrap();
    assert_eq!(jobs.len(), 3, "high + low + idempotent(k1)");
}
