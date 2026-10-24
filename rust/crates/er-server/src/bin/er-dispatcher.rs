//! `er-dispatcher` — the standalone job-dispatcher daemon (a dedicated
//! Deployment). A shim over [`er_server::dispatcher_bin::run`], which is where
//! the wiring lives so the deployed image can host every entry point in a
//! single binary; see `rust/Dockerfile`.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    er_server::dispatcher_bin::run().await
}
