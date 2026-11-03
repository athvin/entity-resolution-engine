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

/// Held for the life of the process. Dropping it releases the process span and
/// flushes whatever the exporter still holds.
#[must_use = "dropping the guard immediately ends the process span; bind it for the life of main"]
pub struct Guard {
    _process: Option<tracing::span::EnteredSpan>,
    provider: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
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
    let provider = crate::otlp::build_provider(role);
    let otel = provider
        .as_ref()
        .map(|provider| tracing_opentelemetry::layer().with_tracer(crate::otlp::tracer(provider)));

    tracing_subscriber::registry()
        .with(filter)
        .with(json)
        .with(otel)
        .init();

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

    Guard {
        _process: Some(process),
        provider,
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
