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
//! Worth keeping in view: this path has the SIGTERM drain and the in-process
//! path does not — `er-server`'s in-process spawn passes a flag that is never
//! set. That difference is why this entry point is preserved rather than
//! deleted along with the Deployment that used to run it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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

    // The attempt ledger's reporter (batch-orchestration-design §11 Phase 1),
    // on its own connection so stage events apply while a run's blocking half
    // is live. Best-effort from birth: no reporter just means no ledger rows.
    let reporter = match crate::reporter::AttemptReporter::connect(&dsn).await {
        Ok(r) => Some(r),
        Err(e) => {
            tracing::warn!(error = %e, "the attempt reporter is disabled: connecting failed");
            None
        }
    };

    // SIGTERM -> drain: finish the in-flight job, then exit at the next loop top.
    let term = Arc::new(AtomicBool::new(false));
    {
        let term = term.clone();
        tokio::spawn(async move {
            if let Ok(mut sig) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                sig.recv().await;
                term.store(true, Ordering::SeqCst);
            }
        });
    }

    // The Kubernetes executor (§6): inert unless the chart lists kinds in
    // ER_EXECUTOR_DAG_KINDS and the pod carries an in-cluster identity.
    let (executor, watch_frames) = crate::executor::from_env(&dsn).await;
    dispatcher::run_loop(&client, &http, &term, reporter, executor, watch_frames).await?;
    Ok(())
}
