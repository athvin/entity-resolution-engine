//! `er-server` binary: bind and serve the control-plane router (P3).

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = std::env::var("ER_SERVER_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("er-server listening on {addr}");
    axum::serve(listener, er_server::app(er_server::state_from_env())).await?;
    Ok(())
}
