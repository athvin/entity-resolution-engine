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
    // `exec-task`: the pod-side verb is synchronous (it owns its own
    // current-thread runtimes), so it also must not run inside the runtime.
    if program_name(&args) != "er" && args.get(1).map(String::as_str) == Some("exec-task") {
        // The guard is bound rather than dropped, so it lives as long as the
        // verb — and this is the flush point, now that there is an exporter.
        // `exec_task::run` funnels all eleven of its early returns through one
        // wrapper, and a pod exits immediately after its last span, so the tail
        // of every run — including every failure, which is the part worth
        // having — depends on flushing here.
        //
        // The flush cannot be left to `Drop`: `process::exit` does not run
        // destructors, so the guard going out of scope here never happens. That
        // is the whole reason this is three statements rather than one.
        let telemetry = er_obs::init(er_obs::Role::Task);
        let code = er_server::exec_task::run(&args[2..]);
        telemetry.force_flush();
        std::process::exit(code);
    }
    // `er`: synchronous, so it must not run inside the runtime. Note that this
    // arm has no tokio runtime at all, which is why `init` must never need one.
    if program_name(&args) == "er" {
        let _telemetry = er_obs::init(er_obs::Role::Cli);
        return er_cli::cli::run().map_err(|e| -> Box<dyn std::error::Error> {
            // anyhow's alternate formatter walks the source chain; `to_string`
            // alone would print only the outermost message.
            format!("{e:#}").into()
        });
    }
    // The API and the standalone dispatcher are the same executable under
    // different names, so the role cannot come from the file name alone.
    let role = if program_name(&args) == "er-dispatcher" {
        er_obs::Role::Dispatcher
    } else {
        er_obs::Role::Api
    };
    let _telemetry = er_obs::init(role);
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
        Some("migrate") => return migrate(&args[2..]).await,
        Some("migrate-tenants") => return migrate_tenants(&args[2..]).await,
        Some("install-extensions") => return install_extensions(&args[2..]),
        _ => {}
    }
    // Assemble every kind's DAG: a graph problem is a refusal to start, not a
    // log line (batch-orchestration-design §3.3).
    er_server::dags::verify_all();
    // Ensure the control-plane schema at boot (it "rides the API boot", as the
    // deploy chart documents, and the init container ran it first). ensure_schema
    // is advisory-locked, so the API and the dispatcher can both ensure it
    // concurrently without deadlock.
    let dsn = std::env::var("ER_PG_DSN").ok().filter(|d| !d.is_empty());
    if let Some(dsn) = &dsn {
        let mut client = er_server::pg::connect(dsn).await?;
        if let Err(e) = er_server::pg::ensure_schema(&mut client).await {
            tracing::error!(error = %er_server::errfmt::chain(&e), "er-server failed to start");
            return Err("the control-plane schema could not be ensured".into());
        }
        if !er_server::pg::ensure_event_triggers(&mut client).await? {
            tracing::warn!(
                "the DDL-ledger event triggers are not installed (requires superuser/rds_superuser); run `er-server migrate` with the master DSN"
            );
        }
    }

    // ER_DISPATCH_IN_PROCESS=1 makes this process also drain the job queue, in a
    // background task.
    //
    // LOCAL DEV ONLY now, and the comment here used to say the opposite — that
    // "the merged web-api service runs the dispatcher in-process" with a
    // separate Deployment as "the alternative topology". The §6.7 split
    // inverted that: the chart ships `er-dispatcher` as its own Deployment,
    // because the identity that creates Jobs must not sit on the pod serving
    // the API behind the BFF. `frontend/dev/env.sh` is the only thing that sets
    // this, where one process is what a developer wants.
    let in_process_dispatcher = if let Some(dsn) =
        dsn.filter(|_| env_flag("ER_DISPATCH_IN_PROCESS"))
    {
        // The drain flag, SHARED with this process's HTTP server: both learn
        // about the same SIGTERM because `tokio::signal` wakes every stream.
        // It used to be a local `AtomicBool::new(false)` named `never` —
        // accurate, since nothing ever set it, so this half of the process had
        // no drain at all while the standalone `er-dispatcher` did.
        let term = er_server::shutdown::sigterm_flag();
        Some(tokio::spawn(async move {
            match er_server::pg::connect(&dsn).await {
                Ok(client) => {
                    let http = er_server::webhook::client();
                    let reporter = match er_server::reporter::AttemptReporter::connect(&dsn).await {
                        Ok(r) => Some(r),
                        Err(e) => {
                            tracing::warn!(error = %e, "the in-process attempt reporter is disabled: connecting failed");
                            None
                        }
                    };
                    let (executor, watch_frames) = er_server::executor::from_env(&dsn).await;
                    if let Err(e) = er_server::dispatcher::run_loop(
                        &client,
                        &http,
                        &term,
                        reporter,
                        executor,
                        watch_frames,
                    )
                    .await
                    {
                        tracing::error!(error = %e, "the in-process dispatcher stopped: control-plane error");
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "the in-process dispatcher could not connect");
                }
            }
        }))
    } else {
        None
    };

    let addr = std::env::var("ER_SERVER_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    // The first structured event in the workspace. The rest of the migration
    // off `eprintln!` is its own change; this one ships here so the subscriber
    // is proven end to end rather than assumed.
    tracing::info!(addr = %addr, "er-server listening");
    let state = er_server::state_from_env().await;
    // Drain on SIGTERM rather than dying mid-response: axum stops accepting and
    // finishes what it already took. The chart pairs this with a `preStop` sleep
    // (endpoint removal propagates asynchronously, so the pod is still being
    // routed to when the signal lands) and a grace period long enough to cover
    // the drain. See `er_server::shutdown` for why SIGINT is deliberately left
    // on its default handler.
    axum::serve(listener, er_server::app(state))
        .with_graceful_shutdown(er_server::shutdown::on_sigterm())
        .await?;

    // Wait for the in-process dispatcher before returning, when there is one.
    //
    // Setting the drain flag is only half a drain: the task is spawned, so
    // `main` returning drops the runtime and kills whatever the dispatcher was
    // mid-way through — the exact outcome the flag exists to prevent. The
    // server's own drain finishes first (above), and the dispatcher's in-flight
    // job is usually the longer of the two.
    //
    // Unbounded on purpose. This path is `ER_DISPATCH_IN_PROCESS`, which only
    // the local dev stack sets (`frontend/dev/env.sh`) — there is no kubelet
    // here to SIGKILL past a grace period, and a dev process that exits when
    // both halves are done is the behaviour a developer wants. The deployed
    // dispatcher is its own Deployment with its own
    // `terminationGracePeriodSeconds`.
    if let Some(handle) = in_process_dispatcher {
        tracing::info!("waiting for the in-process dispatcher to drain");
        if let Err(e) = handle.await {
            tracing::error!(error = %e, "the in-process dispatcher task did not join cleanly");
        }
    }
    Ok(())
}

/// `er-server migrate [--plan [--json]] [--expect-additive]`: the control-plane
/// schema, then the event triggers.
///
/// Three modes, because a deploy gate and a boot want different things:
///
/// - bare — apply, as it has always done.
/// - `--plan` — report what WOULD be applied and write nothing. Exits 3 on a
///   breaking plan, 0 otherwise, so a preflight can branch (see
///   [`er_server::migrate_plan::Verdict::exit_code`]).
/// - `--expect-additive` — plan first, refuse without writing if the plan is
///   breaking, otherwise apply.
///
/// That last mode is not redundant with the refusal `ensure_schema` already
/// carries, and the difference is the whole reason this exists:
/// `ensure_schema` is six transactions, so by the time its phase-3 reconciler
/// refuses, the baseline replay and the `BeforeReconcile` fixups have
/// COMMITTED. Right for a boot — converge as far as is safe, then halt loudly —
/// and wrong for a gate, which must leave the store untouched when it says no.
// The `eprintln!`s in `migrate`, `migrate_tenants` and `install_extensions`
// are DELIBERATE and allowlisted in `structured_logging.rs`. These are operator
// CLI verbs: a human runs `er-server migrate` and reads the result on their
// terminal. Structured JSON is the right shape for a daemon whose output is
// shipped and queried, and the wrong shape for a sentence someone is about to
// read — the same reasoning that exempts `er-entity check` and `er … --json`.
// Everything in `async_main`, which is the daemon path, is `tracing`.
async fn migrate(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    // Parsed in the library so the refusal of an unknown flag is testable; see
    // `migrate_plan::parse_migrate_args` for why silent acceptance is the
    // behaviour being removed.
    let opts = er_server::migrate_plan::parse_migrate_args(args)?;

    let dsn = std::env::var("ER_PG_DSN").map_err(|_| "ER_PG_DSN is required for migrate")?;
    let mut client = er_server::pg::connect(&dsn).await?;

    if opts.plans() {
        let catalog = er_entities::catalog::catalog();
        let relations = er_entity_diff::postgres_relations(&catalog);
        let live = er_entity_diff::read_postgres_columns(&client, &relations).await?;
        let plan = er_entity_diff::plan_migration(&catalog, er_entity_spec::Store::Postgres, &live);
        let hash = er_server::migrate_plan::control_catalog_hash();
        let verdict = er_server::migrate_plan::verdict(&plan);

        if opts.json {
            println!("{}", er_server::migrate_plan::as_json(&plan, &hash));
        } else {
            print!("{}", er_server::migrate_plan::render(&plan, &hash));
        }

        // `--plan` stops here whatever the verdict. `--expect-additive` stops
        // only to refuse, and falls through to apply otherwise.
        if opts.plan_only || verdict == er_server::migrate_plan::Verdict::Breaking {
            std::process::exit(i32::from(verdict.exit_code()));
        }
    }

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
