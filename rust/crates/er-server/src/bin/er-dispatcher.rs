//! `er-dispatcher` — the standalone job-dispatcher daemon (a dedicated
//! Deployment). The loop itself lives in `er_server::dispatcher`, shared with
//! `er-server`'s in-process mode (the merged `web-api` service). This binary
//! wires a Postgres client + an HTTP client + a SIGTERM drain, then runs the
//! loop; a Postgres failure exits non-zero so k8s restarts the pod.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use er_server::{dispatcher, pg};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dsn = std::env::var("ER_PG_DSN").map_err(|_| "ER_PG_DSN is required for the dispatcher")?;
    let mut client = pg::connect(&dsn).await?;
    // Advisory-locked, so it's safe alongside the API's own ensure_schema.
    pg::ensure_schema(&mut client).await?;
    if !pg::ensure_event_triggers(&mut client).await? {
        eprintln!("er-dispatcher: DDL-ledger event triggers not installed (needs superuser)");
    }
    // The dispatcher's writes identify themselves to the history/DDL ledgers.
    pg::set_actor(&client, "er-dispatcher").await?;
    let http = reqwest::Client::new();

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

    dispatcher::run_loop(&client, &http, &term).await?;
    Ok(())
}
