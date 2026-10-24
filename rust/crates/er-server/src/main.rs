//! `er-server` binary: bind and serve the control-plane router (P3), or run one
//! of three deploy-time subcommands and exit:
//!
//! - `migrate` — ensure the control-plane schema (baseline + reconciler) and
//!   install the DDL-ledger event triggers; the web-api pod's init container,
//!   and the one-off master-DSN run the ledger install needs.
//! - `migrate-tenants [--timeout-secs N]` — enqueue one `lake_migrate` job per
//!   active org, wait for the dispatcher to run them, print the per-tenant
//!   report, exit non-zero if any tenant is breaking, errored or timed out;
//!   the chart's post-upgrade hook and its drift CronJob.
//! - `install-extensions --dir PATH` — populate a DuckDB extension directory to
//!   bake into the image. Build-time only; see below.
//!
//! # One binary, three names
//!
//! This binary is a **multicall**: invoked as `er` it is the pipeline CLI, and
//! as `er-dispatcher` it is the standalone dispatcher daemon. The image ships
//! this one executable plus two symlinks, because each of the three used to be
//! its own binary statically linking its own copy of DuckDB — the image paid
//! for DuckDB three times, and only one of the three was ever the entrypoint.
//!
//! Dispatch is on `argv[0]`, which keeps every documented invocation working
//! unchanged (`er ingest …`, `er-server migrate`, `er-dispatcher`) and needs no
//! chart change. All three `[[bin]]` targets still build for local use.
//!
//! The `er` arm runs **before** the tokio runtime starts: the pipeline CLI is
//! synchronous (DuckDB is), and entering it from inside a runtime would put
//! blocking DuckDB work on a reactor thread.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    // `er`: synchronous, so it must not run inside the runtime.
    if program_name(&args) == "er" {
        return er_cli::cli::run().map_err(|e| -> Box<dyn std::error::Error> {
            // anyhow's alternate formatter walks the source chain; `to_string`
            // alone would print only the outermost message.
            format!("{e:#}").into()
        });
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main(args))
}

/// The file name this process was invoked as, with no path and no `.exe`.
fn program_name(args: &[String]) -> String {
    args.first()
        .map(std::path::Path::new)
        .and_then(std::path::Path::file_stem)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

async fn async_main(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    if program_name(&args) == "er-dispatcher" {
        return er_server::dispatcher_bin::run().await;
    }
    match args.get(1).map(String::as_str) {
        Some("migrate") => return migrate().await,
        Some("migrate-tenants") => return migrate_tenants(&args[2..]).await,
        Some("install-extensions") => return install_extensions(&args[2..]),
        _ => {}
    }
    // Ensure the control-plane schema at boot (it "rides the API boot", as the
    // deploy chart documents, and the init container ran it first). ensure_schema
    // is advisory-locked, so the API and the dispatcher can both ensure it
    // concurrently without deadlock.
    let dsn = std::env::var("ER_PG_DSN").ok().filter(|d| !d.is_empty());
    if let Some(dsn) = &dsn {
        let mut client = er_server::pg::connect(dsn).await?;
        if let Err(e) = er_server::pg::ensure_schema(&mut client).await {
            eprintln!("er-server: {}", er_server::errfmt::chain(&e));
            return Err("the control-plane schema could not be ensured".into());
        }
        if !er_server::pg::ensure_event_triggers(&mut client).await? {
            eprintln!(
                "er-server: DDL-ledger event triggers not installed (requires \
                 superuser/rds_superuser); run `er-server migrate` with the master DSN"
            );
        }
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
                    let http = er_server::webhook::client();
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
    let state = er_server::state_from_env().await;
    axum::serve(listener, er_server::app(state)).await?;
    Ok(())
}

/// `er-server migrate`: the control-plane schema, then the event triggers.
async fn migrate() -> Result<(), Box<dyn std::error::Error>> {
    let dsn = std::env::var("ER_PG_DSN").map_err(|_| "ER_PG_DSN is required for migrate")?;
    let mut client = er_server::pg::connect(&dsn).await?;
    if let Err(e) = er_server::pg::ensure_schema(&mut client).await {
        eprintln!("er-server migrate: {}", er_server::errfmt::chain(&e));
        return Err("the control-plane schema could not be migrated".into());
    }
    if er_server::pg::ensure_event_triggers(&mut client).await? {
        eprintln!("er-server: schema reconciled + DDL-ledger event triggers ensured");
    } else {
        eprintln!(
            "er-server: schema reconciled; DDL-ledger event triggers NOT installed \
             (requires superuser/rds_superuser) — run `er-server migrate` with the master DSN"
        );
    }
    Ok(())
}

/// `er-server migrate-tenants [--timeout-secs N]`: the per-tenant sweep.
async fn migrate_tenants(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut timeout_secs: u64 = 1800;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--timeout-secs" => {
                timeout_secs = it
                    .next()
                    .ok_or("--timeout-secs needs a value")?
                    .parse()
                    .map_err(|_| "--timeout-secs must be a whole number of seconds")?;
            }
            other => return Err(format!("unknown argument {other:?}").into()),
        }
    }
    let dsn =
        std::env::var("ER_PG_DSN").map_err(|_| "ER_PG_DSN is required for migrate-tenants")?;
    let client = er_server::pg::connect(&dsn).await?;
    let jobs = er_server::schema::enqueue_tenant_migrations(&client).await?;
    eprintln!(
        "er-server migrate-tenants: {} active tenant(s) enqueued at lake catalog {}",
        jobs.len(),
        &er_server::schema::lake_catalog_hash()[..12]
    );
    let report = er_server::schema::wait_for_tenant_migrations(
        &client,
        &jobs,
        std::time::Duration::from_secs(timeout_secs),
    )
    .await?;
    print!("{}", er_server::schema::render_report(&report));
    let failed = report.iter().filter(|r| r.failed()).count();
    if failed > 0 {
        return Err(format!("{failed} tenant(s) did not reconcile").into());
    }
    Ok(())
}

/// `er-server install-extensions --dir PATH`: populate a DuckDB extension
/// directory, for the image build to bake in.
///
/// This runs from **this** binary on purpose. Extension binaries are keyed by
/// the exact DuckDB version and platform
/// (`<dir>/v<version>/<platform>/<name>.duckdb_extension`), so a directory built
/// by a `duckdb` CLI of a different version, or on a different platform, is
/// silently ignored at runtime — DuckDB would just autoinstall over the network
/// again and the bake would look like it worked. Installing through the same
/// statically-linked DuckDB that will later read the directory removes that
/// whole failure mode, and a `duckdb` version bump cannot leave the two out of
/// step.
///
/// Needs network egress; it is a build-time step, not a runtime one.
fn install_extensions(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut dir: Option<String> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--dir" => dir = Some(it.next().ok_or("--dir needs a value")?.clone()),
            other => return Err(format!("unknown argument {other:?}").into()),
        }
    }
    let dir = dir.ok_or("--dir is required (where to write the extension binaries)")?;
    std::fs::create_dir_all(&dir)?;

    let conn = duckdb::Connection::open_in_memory()?;
    conn.execute_batch(&format!(
        "SET extension_directory = '{}';",
        dir.replace('\'', "''")
    ))?;
    for ext in dedupe_core::session::BAKED_EXTENSIONS {
        // The list is a compile-time constant of bare identifiers, so this
        // interpolation cannot carry anything but one of those names.
        conn.execute_batch(&format!("INSTALL {ext};"))
            .map_err(|e| format!("installing the {ext} extension into {dir}: {e}"))?;
        // Load it too: an INSTALL that wrote an unusable file should fail the
        // build, not the first query in production.
        conn.execute_batch(&format!("LOAD {ext};"))
            .map_err(|e| format!("loading the freshly installed {ext} from {dir}: {e}"))?;
        eprintln!("er-server install-extensions: {ext}");
    }
    eprintln!(
        "er-server install-extensions: {} extension(s) in {dir}",
        dedupe_core::session::BAKED_EXTENSIONS.len()
    );
    Ok(())
}

/// Whether an env flag is set to a truthy value (`1`/`true`/`yes`).
fn env_flag(key: &str) -> bool {
    matches!(
        std::env::var(key).ok().as_deref(),
        Some("1" | "true" | "yes")
    )
}
