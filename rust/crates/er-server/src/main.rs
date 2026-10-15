//! `er-server` binary: bind and serve the control-plane router (P3). The
//! `migrate` subcommand applies the schema and exits (the deploy's db-migrate
//! Job), so a fresh environment has its tables before the API/dispatcher boot.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("migrate") {
        let dsn = std::env::var("ER_PG_DSN").map_err(|_| "ER_PG_DSN is required for migrate")?;
        let mut client = er_server::pg::connect(&dsn).await?;
        er_server::pg::ensure_schema(&mut client).await?;
        eprintln!("er-server: schema ensured");
        return Ok(());
    }
    // Ensure the control-plane schema at boot (it "rides the API boot", as the
    // deploy chart documents). ensure_schema is advisory-locked, so the API and
    // the dispatcher can both ensure it concurrently without deadlock.
    let dsn = std::env::var("ER_PG_DSN").ok().filter(|d| !d.is_empty());
    if let Some(dsn) = &dsn {
        let mut client = er_server::pg::connect(dsn).await?;
        er_server::pg::ensure_schema(&mut client).await?;
    }

    // The merged `web-api` service runs the dispatcher IN-PROCESS: set
    // ER_DISPATCH_IN_PROCESS=1 and the API process also drains the job queue (a
    // background task). At one replica this satisfies the single-dispatcher
    // model; a separate `er-dispatcher` Deployment is the alternative topology.
    if let Some(dsn) = dsn.filter(|_| env_flag("ER_DISPATCH_IN_PROCESS")) {
        tokio::spawn(async move {
            let never = std::sync::atomic::AtomicBool::new(false);
            match er_server::pg::connect(&dsn).await {
                Ok(client) => {
                    let http = reqwest::Client::new();
                    if let Err(e) = er_server::dispatcher::run_loop(&client, &http, &never).await {
                        eprintln!("in-process dispatcher stopped (control-plane): {e}");
                    }
                }
                Err(e) => eprintln!("in-process dispatcher could not connect: {e}"),
            }
        });
    }

    let addr = std::env::var("ER_SERVER_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("er-server listening on {addr}");
    axum::serve(listener, er_server::app(er_server::state_from_env())).await?;
    Ok(())
}

/// Whether an env flag is set to a truthy value (`1`/`true`/`yes`).
fn env_flag(key: &str) -> bool {
    matches!(
        std::env::var(key).ok().as_deref(),
        Some("1" | "true" | "yes")
    )
}
