//! The standalone job-dispatcher daemon's entry point.
//!
//! The loop itself lives in [`crate::dispatcher`], shared with `er-server`'s
//! in-process mode (the merged `web-api` service). This module wires a Postgres
//! client + an HTTP client + a SIGTERM drain, then runs the loop; a Postgres
//! failure returns an error so the caller exits non-zero and k8s restarts the
//! pod.
//!
//! It lives in the library rather than in `src/bin/er-dispatcher.rs` so the
//! deployed image can host it in the same binary as `er-server` and `er`,
//! instead of carrying a third copy of statically-linked DuckDB. The `[[bin]]`
//! target still exists and still builds.
//!
//! The drain is now [`crate::shutdown::sigterm_flag`], shared with the
//! in-process arm rather than written out here. This comment used to record
//! that the two differed — "this path has the SIGTERM drain and the in-process
//! path does not" — because that arm passed a flag nothing ever set. They are
//! the same mechanism now, so the difference it warned about is gone.

use crate::{dispatcher, pg};

/// Run the dispatcher until SIGTERM drains it or the control plane fails.
///
/// # Errors
/// Missing `ER_PG_DSN`, a Postgres connect/schema failure, or a control-plane
/// error from the loop itself.
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Assemble every kind's DAG before touching the queue: a graph problem is
    // a refusal to start (batch-orchestration-design §3.3).
    crate::dags::verify_all();
    let dsn = std::env::var("ER_PG_DSN").map_err(|_| "ER_PG_DSN is required for the dispatcher")?;
    let mut client = pg::connect(&dsn).await?;
    // Advisory-locked, so it's safe alongside the API's own ensure_schema.
    pg::ensure_schema(&mut client).await?;
    if !pg::ensure_event_triggers(&mut client).await? {
        tracing::warn!("the DDL-ledger event triggers are not installed (needs superuser)");
    }
    // The dispatcher's writes identify themselves to the history/DDL ledgers.
    pg::set_actor(&client, "er-dispatcher").await?;
    let http = crate::webhook::client();

    // SIGTERM -> drain: finish the in-flight job, then exit at the next loop
    // top. There is no second copy of this to keep in step any more: the API's
    // in-process arm, whose copy was the one with no drain, is gone (#645).
    let term = crate::shutdown::sigterm_flag();

    // The Kubernetes executor (§6): present when the pod has a namespace and
    // an in-cluster identity. Absent, this dispatcher does not claim — there
    // is nothing left to execute a job with (#645).
    let (executor, watch_frames) = crate::executor::from_env(&dsn).await;
    dispatcher::run_loop(&client, &http, &term, executor, watch_frames).await?;
    Ok(())
}
