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
    if let Ok(dsn) = std::env::var("ER_PG_DSN") {
        if !dsn.is_empty() {
            let mut client = er_server::pg::connect(&dsn).await?;
            er_server::pg::ensure_schema(&mut client).await?;
        }
    }
    let addr = std::env::var("ER_SERVER_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("er-server listening on {addr}");
    axum::serve(listener, er_server::app(er_server::state_from_env())).await?;
    Ok(())
}
