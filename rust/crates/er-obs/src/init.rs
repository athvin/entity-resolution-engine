//! Installing the subscriber: one call at the top of every entry point.
//!
//! Until something installs a subscriber, every span and event this workspace
//! emits — and every one `hyper`, `reqwest`, `tokio-postgres`, `aws-smithy` and
//! `kube` already emit, since `tracing` has been in the lock file all along — is
//! discarded. This module is what starts listening.
//!
//! ## Why there is no OTLP exporter here yet
//!
//! The span pipeline needs a destination, and there is not one: the collector
//! lives at a cluster-internal address, OpenObserve is not stood up, and
//! CloudWatch Transaction Search is not enabled on the account. Shipping an
//! exporter now would add a dependency with nowhere to send.
//!
//! It would also break the image. `opentelemetry-otlp`'s `reqwest-rustls`
//! feature resolves a **second** `reqwest` (0.13 beside the workspace's 0.12)
//! and selects `aws-lc-rs` as rustls' crypto provider, which pulls
//! `aws-lc-sys` — and `aws-lc-sys` needs a cmake the `rust:1-bookworm` build
//! image does not carry. Measured on this branch: adding it took the lock from
//! zero `aws-lc-sys` entries to one and from one `reqwest` to two. That is the
//! pin `rust-conventions` says must not be relaxed, so the exporter lands with
//! the collector instead, over the `reqwest` the workspace already links.
//!
//! What ships now is the half that needs none of that and works today: one
//! structured JSON object per event on **stderr**, which Fluent Bit already
//! tails into CloudWatch from every pod. [`Guard`] exists in its final shape so
//! the exporter can be added later without touching a single call site.

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

/// Held for the life of the process. Dropping it releases the process span.
///
/// Deliberately shaped for a flush it cannot yet perform: when the OTLP
/// exporter lands, [`Guard::force_flush`] gains a body and `Drop` gains a
/// shutdown, and no call site changes. That matters most for the pod verb,
/// where the batch exporter drops unflushed spans at process exit and the tail
/// of every run — including every failure — would vanish without an explicit
/// flush on each exit path.
#[must_use = "dropping the guard immediately ends the process span; bind it for the life of main"]
pub struct Guard {
    _process: Option<tracing::span::EnteredSpan>,
}

impl Guard {
    /// Export everything buffered, blocking up to `ER_OTEL_FLUSH_SECS`.
    ///
    /// A no-op while there is no exporter. Call it anyway on every path that
    /// may `process::exit` or hang — the call sites are the hard part to get
    /// right, not the flush, and they are easier to add now than to find later.
    pub fn force_flush(&self) {}
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
        return Guard { _process: None };
    }

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    let json = tracing_subscriber::fmt::layer()
        .json()
        .flatten_event(true)
        .with_current_span(true)
        .with_span_list(false)
        .with_target(true)
        .with_writer(std::io::stderr);

    tracing_subscriber::registry()
        .with(filter)
        .with(json)
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
