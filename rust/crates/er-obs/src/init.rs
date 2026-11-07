//! Installing the subscriber: one call at the top of every entry point.
//!
//! Until something installs a subscriber, every span and event this workspace
//! emits — and every one `hyper`, `reqwest`, `tokio-postgres`, `aws-smithy` and
//! `kube` already emit, since `tracing` has been in the lock file all along — is
//! discarded. This module is what starts listening.
//!
//! ## Two layers, and why the second one is conditional
//!
//! Always: one structured JSON object per event on **stderr**, which Fluent Bit
//! already tails into CloudWatch from every pod.
//!
//! Additionally, when an OTLP endpoint is configured: a
//! `tracing-opentelemetry` bridge that exports the same spans over HTTP. The
//! condition is the whole design — `er-entity` and `dedupe` are build tools and
//! the test binaries run thousands of times locally, so an exporter must be
//! opt-in by the presence of [`crate::traces_endpoint`] rather than opt-out by
//! a flag. Nothing opens a socket unless something asked it to.
//!
//! An earlier revision of this module explained at length why the exporter
//! could not be added: `opentelemetry-otlp`'s `reqwest-rustls` feature pulls a
//! second `reqwest` and `aws-lc-sys`, which needs a cmake the build image lacks.
//! That measurement was right about the feature it tested and wrong as a verdict
//! on the exporter. Supplying our own HTTP client avoids the client features
//! entirely; [`mod@crate::otlp`] carries the re-measurement and the reactor trap
//! that shapes it.

use std::sync::OnceLock;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Which process this is. One binary serves several of these: `er-server` is a
/// multicall dispatching on `argv[0]`, so the role is not derivable from the
/// executable name alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The control-plane API (and, under `ER_DISPATCH_IN_PROCESS`, a dispatcher
    /// beside it).
    Api,
    /// The standalone dispatcher daemon.
    Dispatcher,
    /// The pod-side `exec-task` verb: one attempt, then exit.
    Task,
    /// The `er` pipeline CLI.
    Cli,
    /// `er-entity` — a build tool, not a deployable.
    EntityCli,
    /// `dedupe` — the engine's own CLI.
    Dedupe,
    /// `er-mcp`, whose **stdout is JSON-RPC framing**. Nothing may write there.
    ///
    /// **Local-only, by decision rather than by omission.** This role ships as
    /// no workload — no chart, no image target, no Dockerfile — and it must
    /// stay that way. Two independent reasons:
    ///
    /// - It speaks JSON-RPC over **stdio**, which is what a local MCP client
    ///   expects. Deployed to the cluster there is nothing attached to its
    ///   stdin, so it would serve no one.
    /// - `ER_MCP_API_KEY` is **one org's** key and there is no operator mode,
    ///   deliberately. A shared deployment of a single-tenant credential is a
    ///   tenant-isolation bug, not a feature.
    ///
    /// Recorded here because the absence is otherwise indistinguishable from an
    /// oversight, and the obvious "fix" — give it a Deployment — is the wrong
    /// one. The generated roles table in `.claude/skills/observability` derives
    /// which roles ship, so this cell cannot quietly flip.
    Mcp,
}

impl Role {
    /// The `service.name` this role reports as.
    #[must_use]
    pub const fn service_name(self) -> &'static str {
        match self {
            Self::Api => "er-api",
            Self::Dispatcher => "er-dispatcher",
            Self::Task => "er-task",
            Self::Cli => "er-cli",
            Self::EntityCli => "er-entity",
            Self::Dedupe => "dedupe",
            Self::Mcp => "er-mcp",
        }
    }

    /// Whether this role runs **unattended** — with nobody reading its stderr
    /// as the output of a command they just typed.
    ///
    /// This decides where a diagnostic belongs, and it is the one question
    /// `eprintln!` versus `tracing::warn!` actually turns on:
    ///
    /// - **Unattended.** Nobody is watching. stderr is consumed by something
    ///   else later — a log shipper, a host process — so a diagnostic has to be
    ///   a structured event: it needs span ancestry to be attributable to a run
    ///   or a tenant, and a target so the default filter can hold its volume
    ///   down. A bare `eprintln!` here is a line nobody can group, filter or
    ///   join to a trace.
    /// - **Attended.** stderr **is the product**. `er-entity check`'s output is
    ///   asserted by tests, `er-entity lint` prints diagnostics a human acts
    ///   on, and `dedupe`'s "wrote N rows to …" is read by whoever typed the
    ///   command. Routing those through the subscriber would wrap them in JSON
    ///   and put them behind `RUST_LOG`, where a narrow filter makes them
    ///   vanish. `eprintln!` is correct, and converting it is a regression.
    ///
    /// ## Why not `is_deployed`, which this was first called
    ///
    /// Because that name is checkably false for [`Role::Mcp`], and the original
    /// justification for it — "stderr is read later by an operator, through
    /// Fluent Bit and CloudWatch" — describes only the pod roles. **`er-mcp` is
    /// not deployed at all:** it appears nowhere in `infra/`, is not an image
    /// target, and is in no Dockerfile. An MCP client spawns it as a stdio
    /// subprocess, so there is no cluster and no Fluent Bit in its path.
    ///
    /// It still belongs on this side, which is the point: its stderr is
    /// surfaced by the host client rather than read as a command's output, and
    /// its parse-rejection path fires on untrusted input at runtime. So
    /// "unattended" and "deployed" are different questions, and only the first
    /// one predicts the right logging idiom.
    ///
    /// Stated as a value rather than left to this module's prose because
    /// `.claude/skills/observability` derives its structured-logging gap from
    /// these arms. The first version of that gap counted every file with an
    /// `eprintln!` and no `tracing` event, which named six files of correct
    /// attended output as if they were defects — a measurement that would have
    /// driven exactly the wrong change.
    #[must_use]
    pub const fn is_unattended(self) -> bool {
        match self {
            Self::Api | Self::Dispatcher | Self::Task | Self::Mcp => true,
            Self::Cli | Self::EntityCli | Self::Dedupe => false,
        }
    }

    /// The short `er.role` dimension.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Dispatcher => "dispatcher",
            Self::Task => "task",
            Self::Cli => "cli",
            Self::EntityCli => "entity-cli",
            Self::Dedupe => "dedupe",
            Self::Mcp => "mcp",
        }
    }
}

/// The filter applied when `RUST_LOG` is unset.
///
/// **This is a cost control, not a preference.** `tracing` is already in the
/// lock file by way of `hyper`, `h2`, `reqwest`, `tokio-postgres`,
/// `aws-smithy-runtime` and `kube`, all of it currently discarded for want of a
/// subscriber. The moment one is installed, `kube`'s watch loop and `hyper`'s
/// per-connection events start flowing — and CloudWatch Logs bills by the
/// gigabyte. So the dependency targets are pinned down in the same change that
/// starts listening to them, rather than discovered on the first deploy.
///
/// Our own crates sit at `debug`, which is where "lean towards over-logging"
/// cashes in: the dispatcher narrates itself, and the lever against volume is
/// this filter rather than the absence of the call.
pub const DEFAULT_FILTER: &str = concat!(
    "info",
    ",er_server=debug,er_cli=debug,er_obs=debug,er_span=debug",
    ",dedupe_core=debug,er_ingest=debug,er_standardize=debug",
    ",er_match=debug,er_reconcile=debug,er_golden=debug",
    // Third-party chatter, held down deliberately. kube at info still reports
    // reconcile outcomes; at debug it reports every watch event.
    ",hyper=warn,hyper_util=warn,h2=warn,rustls=warn,reqwest=warn",
    ",tokio_postgres=warn,aws_smithy_runtime=warn,aws_config=warn",
    ",aws_sdk_sesv2=warn,tower=warn,kube=info,kube_client=warn",
);

/// A per-layer filter that passes spans and drops **events**.
///
/// ## Why the exported signal carries no events
///
/// `tracing-opentelemetry` turns every `tracing` event inside a span into a
/// span event, fields and all — so any `tracing::error!(error = %e)` anywhere
/// in the workspace becomes exported content. That is an uncontrolled surface:
/// a database error message routinely contains the offending record value.
/// Measured on DuckDB, with a real fixture address:
///
/// ```text
/// Constraint Error: Duplicate key "email: robert.chen@example.com"
/// violates primary key constraint.
/// ```
///
/// §10.4's rule is that telemetry carries identifiers and counts and never
/// values, and this is a deduplication engine, so that string IS the payload.
///
/// Auditing the call sites would not hold: the workspace keeps adding events,
/// and the next person to write one cannot be expected to know that an `error`
/// field reaches an exportable store. So the guarantee is structural, the same
/// way `er_span::sql_span` has no parameter argument: events are not exported at
/// all, and no field of one can leak.
///
/// This sentence used to say "~118 `eprintln!`s still to become `tracing`
/// events", which was wrong twice over and is worth not restoring. The count was
/// stale, and the premise was false: most of those are an **interactive** role's
/// output, where stderr is the product and converting it is a regression — see
/// [`Role::is_unattended`], which is where that distinction is a value rather
/// than a claim in prose.
///
/// Nothing is lost in-account. Every event still reaches stderr through the
/// structured JSON layer and Fluent Bit still ships it to CloudWatch, which is
/// where a message belongs. The span keeps the FACT of a failure —
/// `er.outcome` and `otel.status_code` — which is what the collector's `errors`
/// sampling policy and every dashboard actually read.
///
/// Reversible by deleting one `with_filter`, if a span-event allowlist ever
/// becomes worth building.
pub struct SpansOnly;

impl<S> tracing_subscriber::layer::Filter<S> for SpansOnly {
    fn enabled(
        &self,
        _meta: &tracing::Metadata<'_>,
        _cx: &tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        true
    }

    fn event_enabled(
        &self,
        _event: &tracing::Event<'_>,
        _cx: &tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        false
    }
}

/// Held for the life of the process. Dropping it releases the process span and
/// flushes whatever the exporter still holds.
#[must_use = "dropping the guard immediately ends the process span; bind it for the life of main"]
pub struct Guard {
    _process: Option<tracing::span::EnteredSpan>,
    provider: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
    /// The meter provider, held for the same reason as the tracer's: a
    /// `PeriodicReader` exports on an interval, so a process that exits inside
    /// one loses whatever it accumulated unless something flushes on the way
    /// out. For a runner pod — which exits as soon as its last stage finishes
    /// — that is usually the entire run.
    meters: Option<opentelemetry_sdk::metrics::SdkMeterProvider>,
}

impl Guard {
    /// Export everything buffered, blocking up to `ER_OTEL_FLUSH_SECS`.
    ///
    /// A no-op when no exporter is installed, so it is always safe to call.
    /// Call it on **every** path that may `process::exit`: the batch exporter
    /// drops whatever it still holds when the process ends, and a runner pod
    /// exits immediately after its last span, so the tail of every run —
    /// including every failure, which is the part worth having — would vanish.
    ///
    /// `exec_task::run` is the single wrapper all eleven of the pod verb's
    /// early returns funnel through, which is why covering them costs one call
    /// rather than eleven.
    ///
    /// SIGKILL and OOM cannot be covered by anything here. The terminal message
    /// and stdout→Fluent Bit are that path.
    pub fn force_flush(&self) {
        if let Some(provider) = &self.provider {
            // Deliberately not propagated. A failed flush must not change a
            // run's outcome, and the one thing worse than losing the tail of a
            // trace is failing a job because telemetry could not be delivered.
            if let Err(error) = provider.force_flush() {
                tracing::debug!(error = %error, "flushing buffered spans failed");
            }
        }
        if let Some(meters) = &self.meters
            && let Err(error) = meters.force_flush()
        {
            tracing::debug!(error = %error, "flushing buffered metrics failed");
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        // Flush before shutdown, and both before the process span's own `Drop`
        // runs — struct fields drop after the body, so the ancestry is still
        // intact for anything emitted here.
        self.force_flush();
        if let Some(provider) = &self.provider
            && let Err(error) = provider.shutdown()
        {
            tracing::debug!(error = %error, "shutting down the span exporter failed");
        }
        if let Some(meters) = &self.meters
            && let Err(error) = meters.shutdown()
        {
            tracing::debug!(error = %error, "shutting down the metric exporter failed");
        }
    }
}

/// Install the subscriber. Idempotent: a second call returns an inert guard.
///
/// Safe as the **first statement** of `main`, and that is where it belongs —
/// it reads only the environment, and several binaries' earliest diagnostics
/// (a missing `ER_PG_DSN`, an unparseable `--timeout-secs`) happen before
/// anything else and would otherwise be emitted into a void.
///
/// Writes to **stderr**, never stdout. Three separate contracts depend on that:
/// `er … --json` is parsed by callers, `er-entity check`'s output is asserted
/// by tests, and `er-mcp` speaks JSON-RPC on stdout, where one stray line
/// corrupts the protocol.
pub fn init(role: Role) -> Guard {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    if INSTALLED.set(()).is_err() {
        // Already installed. Returning an inert guard rather than panicking is
        // what makes this safe to call from a multicall arm that may be reached
        // twice, and from a test binary whose modules are threads in one
        // process — `er-server`'s ~50 test modules share a process, so a second
        // install would poison the first.
        //
        // The inert guard carries no provider, so its `force_flush` and `Drop`
        // do nothing. That matters: the first caller owns the exporter, and a
        // second guard going out of scope must not shut it down underneath it.
        return Guard {
            _process: None,
            provider: None,
            meters: None,
        };
    }

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    // `with_span_list` is what makes a record self-describing. Without it a
    // line carries only its innermost span, so a statement-level event says
    // which stage it was in and nothing about which run, job, tenant or dataset
    // — which is the attribution the whole exercise exists for. With it, every
    // record carries its full ancestry: process -> job -> run -> stage.
    //
    // It costs bytes, and the lever against that is the filter rather than
    // throwing away the ancestry, because a cheap line nobody can attribute is
    // not actually cheap.
    let json = tracing_subscriber::fmt::layer()
        .json()
        .flatten_event(true)
        .with_current_span(true)
        .with_span_list(true)
        .with_target(true)
        .with_writer(std::io::stderr);

    // `Option<Layer>` is itself a `Layer`, so the exporter is absent from the
    // subscriber entirely rather than installed-and-disabled when no endpoint
    // is set. A disabled layer still costs a per-span branch, and this path runs
    // in every test binary in the workspace.
    // Built BEFORE the subscriber, because the provider becomes one of its
    // layers — which is why its diagnostics are deferred to `report()` below
    // rather than emitted here, where nothing would be listening yet.
    let outcome = crate::otlp::build_provider(role);
    let otel = outcome.provider.as_ref().map(|provider| {
        tracing_opentelemetry::layer()
            .with_tracer(crate::otlp::tracer(provider))
            // Spans leave the process; events do not. See `SpansOnly`.
            .with_filter(SpansOnly)
    });

    tracing_subscriber::registry()
        .with(filter)
        .with(json)
        .with(otel)
        // Inert until a sink is installed, which only the pipeline does — so
        // this is free in `er-entity`, `dedupe` and every test binary.
        .with(crate::executions::ExecutionRecorder)
        .init();

    // Now there is somewhere for it to go.
    outcome.report(role);

    // A process-lifetime span carries the service identity onto every record,
    // which `with_current_span` then includes. Pod identity is not duplicated
    // here: Fluent Bit's Kubernetes filter already attaches namespace, pod and
    // container to every line it ships, and repeating it would pay for the same
    // fields twice at $0.50/GB.
    let process = tracing::info_span!(
        "process",
        service.name = role.service_name(),
        service.version = env!("CARGO_PKG_VERSION"),
        er.role = role.as_str(),
    )
    .entered();

    // Metrics are built AFTER the subscriber, unlike traces, because nothing
    // about them has to become a layer of it — so the failure path can simply
    // log, and does not need `Outcome`'s report-it-later dance.
    //
    // Same endpoint as the spans: the collector's `otlp` receiver serves all
    // signals on 4318, and the plan's one-store decision means there is nothing
    // to point elsewhere.
    let meters = match crate::traces_endpoint() {
        Some(endpoint) => {
            let signal = endpoint.replace("/v1/traces", "/v1/metrics");
            match crate::otlp::meter_provider_for(role, &signal, crate::flush_timeout()) {
                Ok(provider) => {
                    opentelemetry::global::set_meter_provider(provider.clone());
                    crate::metrics::install_from_global();
                    tracing::debug!(endpoint = %signal, role = role.as_str(), "OTLP metric export installed");
                    Some(provider)
                }
                Err(error) => {
                    tracing::warn!(error = %error, "building the metric exporter failed; metrics are disabled");
                    None
                }
            }
        }
        None => None,
    };

    Guard {
        _process: Some(process),
        provider: outcome.provider,
        meters,
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_FILTER, Role};

    #[test]
    fn every_role_names_itself_distinctly() {
        let roles = [
            Role::Api,
            Role::Dispatcher,
            Role::Task,
            Role::Cli,
            Role::EntityCli,
            Role::Dedupe,
            Role::Mcp,
        ];
        let services: std::collections::BTreeSet<_> =
            roles.iter().map(|r| r.service_name()).collect();
        let shorts: std::collections::BTreeSet<_> = roles.iter().map(|r| r.as_str()).collect();
        assert_eq!(
            services.len(),
            roles.len(),
            "service names must be distinct"
        );
        assert_eq!(
            shorts.len(),
            roles.len(),
            "role dimensions must be distinct"
        );
    }

    #[test]
    fn the_default_filter_parses_and_pins_the_noisy_dependencies() {
        // A filter that does not parse falls back to `info` for everything,
        // which is precisely the volume this directive exists to prevent — and
        // it would fail silently.
        let filter: tracing_subscriber::EnvFilter = DEFAULT_FILTER.parse().expect("parses");
        drop(filter);

        // These are the crates already emitting into a void today. If one is
        // dropped from the directive, it is held at `info` on the next deploy
        // and bills by the gigabyte.
        for dep in [
            "hyper=warn",
            "h2=warn",
            "rustls=warn",
            "reqwest=warn",
            "tokio_postgres=warn",
            "aws_smithy_runtime=warn",
            "kube=info",
        ] {
            assert!(
                DEFAULT_FILTER.contains(dep),
                "{dep} must stay pinned in the default filter"
            );
        }
    }

    #[test]
    fn the_unattended_roles_are_the_ones_nobody_is_watching() {
        // Two arms are worth pinning for opposite reasons.
        //
        // `Task` because the obvious shortcut is wrong: `Role::Cli` is the `er`
        // pipeline CLI a human invokes (`er_cli::cli::run()` parses argv) while
        // the pod runs `exec-task` as `Role::Task`, so "is this the engine?" is
        // NOT the question and classifying by crate would misfile it.
        //
        // `Mcp` because it is unattended WITHOUT being deployed — nothing in
        // `infra/`, no image target, no Dockerfile — which is exactly why this
        // predicate is not called `is_deployed`.
        for unattended in [Role::Api, Role::Dispatcher, Role::Task, Role::Mcp] {
            assert!(
                unattended.is_unattended(),
                "{} runs unattended: nobody reads its stderr as the output of a \
                 command they typed, so its diagnostics must be structured",
                unattended.service_name()
            );
        }
        for attended in [Role::Cli, Role::EntityCli, Role::Dedupe] {
            assert!(
                !attended.is_unattended(),
                "{} is attended: its stderr IS its product, so `eprintln!` is \
                 correct there and routing it through the subscriber would put \
                 it behind RUST_LOG",
                attended.service_name()
            );
        }
    }

    #[test]
    fn init_is_idempotent() {
        // The second call must not panic: `er-server`'s test binary runs ~50
        // modules as threads in ONE process, so any module calling init would
        // otherwise poison every other.
        let a = super::init(Role::Cli);
        let b = super::init(Role::Api);
        drop(b);
        drop(a);
    }
}
