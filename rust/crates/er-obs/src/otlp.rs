//! The OTLP span exporter, over the `reqwest` the workspace already links.
//!
//! ## Why this module exists rather than a feature flag
//!
//! `opentelemetry-otlp` can bring its own HTTP client, and that is the path this
//! workspace cannot take. Its `reqwest-rustls` feature resolves a **second**
//! `reqwest` beside the workspace's 0.12 and selects `aws-lc-rs` as rustls'
//! crypto provider, which pulls `aws-lc-sys` — and `aws-lc-sys` needs a cmake
//! the `rust:1-bookworm` build image does not carry. That is the pin
//! `rust-conventions` says must not be relaxed.
//!
//! Supplying the client instead keeps the resolution clean. Measured on this
//! branch with `default-features = false` and `http-proto`: still **zero**
//! `aws-lc-sys` entries, still **one** `reqwest` (0.12.28), rustls still on
//! `ring`, and twelve new lock entries none of which is a second HTTP or TLS
//! stack. `tonic` is among them — `opentelemetry-proto` depends on it
//! unconditionally — but without its `transport` feature, which is why no new
//! `hyper` appears. It is a protobuf codec here, not a gRPC stack.
//!
//! ## The trap this module is shaped around
//!
//! The SDK's batch processor owns a plain `std::thread` and polls the exporter
//! future on it with a non-tokio executor. An async `reqwest::Client` needs a
//! tokio reactor, so sending directly from that thread panics:
//!
//! ```text
//! thread 'OpenTelemetry.Traces.BatchProcessor' panicked:
//! there is no reactor running, must be called from the context of a Tokio 1.x runtime
//! ```
//!
//! **That panic does not fail the process.** It happens on the processor's own
//! thread, so `main` runs on and every span is silently dropped — the failure
//! mode is a telemetry pipeline that looks installed and exports nothing. It is
//! also invisible to a build-only check, which is why
//! `tests/otlp_export.rs` sends a real span to a real socket.
//!
//! [`ReqwestClient`] therefore hands each request to a runtime this module
//! owns. One worker thread, built once, never dropped. That works whether or
//! not the calling process has a runtime of its own: `er-server`'s API and
//! dispatcher arms are `#[tokio::main]`, while the `er` pipeline CLI and the
//! `exec-task` pod verb deliberately are not — DuckDB is synchronous and
//! `er-server/src/main.rs` says entering a runtime would put blocking work on a
//! reactor thread. Both are exercised in the tests.

use std::sync::OnceLock;
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_http::{Bytes, HttpClient, HttpError, Request, Response};
use opentelemetry_otlp::{SpanExporter, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;

use crate::init::Role;

/// How long a single export attempt may take.
const DEFAULT_TIMEOUT_SECS: u64 = 10;

/// The runtime the exporter's requests run on.
///
/// A dedicated single-worker runtime rather than the caller's, for two
/// independent reasons. Half this workspace's roles have no runtime at all, so
/// there may be nothing to borrow. And where there is one, telemetry should not
/// queue behind the request handling it is measuring — an exporter that gets
/// slower exactly when the system is busy is the failure mode `D-OTEL` names.
///
/// Never dropped. Dropping a `Runtime` from inside another runtime's context
/// panics, and a process-lifetime static cannot be dropped at the wrong moment.
fn export_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("er-otel-export")
            .build()
            .expect("building the OTLP exporter runtime")
    })
}

/// An [`HttpClient`] over the workspace `reqwest`.
///
/// Each request is handed to a single-worker runtime this module owns, so the
/// client works whether or not the calling process has a runtime of its own.
/// See the module documentation for why that indirection is load-bearing.
#[derive(Debug)]
pub struct ReqwestClient(reqwest::Client);

impl ReqwestClient {
    /// A client with the export timeout already applied.
    fn new(timeout: Duration) -> Result<Self, reqwest::Error> {
        reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map(Self)
    }
}

#[async_trait::async_trait]
impl HttpClient for ReqwestClient {
    async fn send_bytes(&self, request: Request<Bytes>) -> Result<Response<Bytes>, HttpError> {
        let client = self.0.clone();
        // `spawn` rather than `block_on`, and the reason is narrower than it
        // looks. Today the SDK always polls this on the batch processor's own
        // thread, which has no ambient runtime, so `block_on` would work too —
        // measured, not assumed. `spawn` is used because it does not depend on
        // that: if a future SDK version ever polls the exporter on the caller's
        // runtime, `block_on` becomes a panic inside a runtime context while
        // `spawn` keeps working. Same cost, one fewer assumption.
        export_runtime()
            .spawn(async move {
                let (parts, body) = request.into_parts();
                let response = client
                    .request(parts.method, parts.uri.to_string())
                    .headers(parts.headers)
                    .body(body)
                    .send()
                    .await?;

                let status = response.status();
                let body = response.bytes().await?;
                // Only the status is read back: the OTLP response body carries
                // partial-success detail, and the builder cannot fail here
                // because a status that parsed off the wire is always valid.
                Ok::<_, reqwest::Error>(
                    Response::builder()
                        .status(status)
                        .body(body)
                        .expect("a wire status is always a valid response status"),
                )
            })
            .await
            .map_err(|join| Box::new(join) as HttpError)?
            .map_err(|send| Box::new(send) as HttpError)
    }
}

/// The traces endpoint, or `None` when no exporter should be installed.
///
/// Absence is the normal case and must stay cheap: `er-entity` and `dedupe` are
/// build tools, tests run thousands of times locally, and none of them should
/// open a socket. So the exporter is opt-in by the presence of an endpoint
/// rather than opt-out by a flag.
///
/// Follows the OTLP environment-variable spec: the signal-specific
/// `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` is used verbatim, while the general
/// `OTEL_EXPORTER_OTLP_ENDPOINT` is a base that the signal path is appended to.
/// Getting that backwards is a silent 404 per batch.
#[must_use]
pub fn traces_endpoint() -> Option<String> {
    resolve_traces_endpoint(
        non_empty("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").as_deref(),
        non_empty("OTEL_EXPORTER_OTLP_ENDPOINT").as_deref(),
    )
}

/// The endpoint rule, separated from the environment so it can be tested.
///
/// The workspace denies `unsafe_code` and `std::env::set_var` is unsafe as of
/// edition 2024, so a test cannot set these variables. That is the better
/// arrangement regardless: `er-obs`'s test modules share one binary, and a test
/// that mutates process-wide state is a test whose result depends on what ran
/// beside it.
fn resolve_traces_endpoint(signal: Option<&str>, base: Option<&str>) -> Option<String> {
    if let Some(signal) = signal {
        return Some(signal.to_string());
    }
    Some(format!("{}/v1/traces", base?.trim_end_matches('/')))
}

/// A variable's value, with blank treated as absent.
///
/// An unset Helm value renders as the empty string rather than vanishing, so
/// `""` has to read as "not configured" or the exporter tries to post to an
/// endpoint of nothing.
fn non_empty(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(|v| clean(&v))
}

fn clean(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The environment this process reports as, for the one-store/one-field split.
///
/// Every environment ships to the same OpenObserve deployment, so
/// `deployment.environment.name` is what separates them. `ERSERVER_ENV` is the
/// chart's existing value (`.Values.environment`); a process with none is
/// genuinely not deployed, so it says so rather than claiming to be `dev` and
/// polluting dev's streams from a laptop.
fn environment_name() -> String {
    non_empty("ERSERVER_ENV").unwrap_or_else(|| "local".to_string())
}

/// The product this process belongs to, within one shared store.
///
/// The chart sets `OTEL_SERVICE_NAMESPACE` on both the deployments and the task
/// template. That is **not** an OTLP SDK variable — the spec carries
/// `service.namespace` inside `OTEL_RESOURCE_ATTRIBUTES` — so nothing read it
/// until this function did, and it was a value shipped to no effect. Honouring
/// the name the chart already uses is cheaper than changing the chart and
/// leaves one spelling rather than two.
fn service_namespace() -> String {
    non_empty("OTEL_SERVICE_NAMESPACE").unwrap_or_else(|| "er".to_string())
}

/// How long [`crate::Guard::force_flush`] may block.
///
/// Five seconds by default, which has to sit inside both the chart's 120 s
/// termination grace period and the dispatcher's 10 s report timeout — a flush
/// that outlives either turns a clean shutdown into a kill.
#[must_use]
pub fn flush_timeout() -> Duration {
    parse_secs(non_empty("ER_OTEL_FLUSH_SECS").as_deref(), 5)
}

/// Seconds from a string, falling back rather than failing.
///
/// This runs from the first statement of `main`, where a panic is a pod that
/// will not boot for a reason unrelated to its job. An unparseable value is a
/// typo in a Helm value, not a reason to refuse to start.
fn parse_secs(raw: Option<&str>, default: u64) -> Duration {
    Duration::from_secs(raw.and_then(|v| v.parse().ok()).unwrap_or(default))
}

/// Build a provider exporting to an explicit endpoint.
///
/// The endpoint is a parameter rather than read from the environment, and that
/// is what makes this testable: the workspace denies `unsafe_code` so a test
/// cannot set `OTEL_EXPORTER_OTLP_ENDPOINT`, and a build-only test of this path
/// would miss the reactor trap described at the top of this module entirely.
/// `tests/otlp_export.rs` calls this with a real socket.
///
/// # Errors
///
/// If the HTTP client or the exporter cannot be constructed — a malformed
/// endpoint, or a TLS backend that will not initialise.
pub fn provider_for(
    role: Role,
    endpoint: &str,
    timeout: Duration,
) -> Result<SdkTracerProvider, Box<dyn std::error::Error + Send + Sync>> {
    let client = ReqwestClient::new(timeout)?;
    let exporter = SpanExporter::builder()
        .with_http()
        .with_http_client(client)
        .with_endpoint(endpoint)
        .with_timeout(timeout)
        .build()?;

    // No sampler is configured, which means `always_on`, and that is deliberate.
    // The sampling decision belongs to the collector's `tail_sampling`
    // processor, which can see a trace's outcome — an error, a slow request, an
    // impersonation — before deciding. A head sampler here would throw away
    // exactly the traces worth keeping, before anything knows they matter.
    Ok(SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(
            Resource::builder()
                .with_service_name(role.service_name())
                .with_attributes([
                    KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
                    KeyValue::new("service.namespace", service_namespace()),
                    KeyValue::new("deployment.environment.name", environment_name()),
                    KeyValue::new("er.role", role.as_str()),
                ])
                .build(),
        )
        .build())
}

/// What `build_provider` decided, so it can be REPORTED after the subscriber
/// exists rather than into a void.
///
/// This type exists because of a bug it now prevents. `build_provider` is
/// called from `init` BEFORE `tracing_subscriber::...init()` — it has to be,
/// since the provider becomes a layer of that subscriber — so every
/// `tracing::debug!`/`warn!` it emitted went to a subscriber that did not exist
/// yet and was silently discarded. The practical cost was real: with an
/// endpoint set and nothing arriving at the collector, there was no way to tell
/// "no endpoint configured" from "the exporter failed to build" from "it built
/// and the spans went somewhere else". Found while verifying the pipeline
/// end to end, which is exactly when a silent diagnostic hurts most.
pub(crate) struct Outcome {
    pub provider: Option<SdkTracerProvider>,
    /// The endpoint resolved, if any. `None` means none was configured.
    endpoint: Option<String>,
    /// Why the build failed, when it did.
    error: Option<String>,
}

impl Outcome {
    /// Emit what was decided. Call AFTER the subscriber is installed.
    pub fn report(&self, role: Role) {
        match (&self.endpoint, &self.error) {
            (Some(endpoint), None) => tracing::debug!(
                endpoint = %endpoint,
                role = role.as_str(),
                "OTLP span export installed"
            ),
            (Some(endpoint), Some(error)) => tracing::warn!(
                endpoint = %endpoint,
                error = %error,
                "building the OTLP span exporter failed; traces will not be exported"
            ),
            // No endpoint is the normal case for a build tool or a test binary,
            // and saying so on every invocation would be noise.
            (None, _) => {}
        }
    }
}

/// Build the tracer provider, or `None` when no endpoint is configured.
///
/// Failures degrade to "no telemetry" rather than propagating: observation must
/// never change a run — the same contract `reporter.rs` states for itself — and
/// this is called from the first statement of `main`, where returning an error
/// means a pod that will not boot for a reason unrelated to its job.
///
/// Emits nothing itself; see [`Outcome`].
pub(crate) fn build_provider(role: Role) -> Outcome {
    let Some(endpoint) = traces_endpoint() else {
        return Outcome {
            provider: None,
            endpoint: None,
            error: None,
        };
    };
    let timeout = parse_secs(
        non_empty("ER_OTEL_TIMEOUT_SECS").as_deref(),
        DEFAULT_TIMEOUT_SECS,
    );

    match provider_for(role, &endpoint, timeout) {
        Ok(provider) => Outcome {
            provider: Some(provider),
            endpoint: Some(endpoint),
            error: None,
        },
        Err(error) => Outcome {
            provider: None,
            endpoint: Some(endpoint),
            error: Some(error.to_string()),
        },
    }
}

/// The tracer the `tracing` bridge layer records through.
pub(crate) fn tracer(provider: &SdkTracerProvider) -> opentelemetry_sdk::trace::SdkTracer {
    provider.tracer("er")
}

#[cfg(test)]
mod tests {
    use super::{clean, parse_secs, resolve_traces_endpoint};

    #[test]
    fn the_general_endpoint_is_a_base_and_takes_the_signal_path() {
        assert_eq!(
            resolve_traces_endpoint(None, Some("http://collector:4318")).as_deref(),
            Some("http://collector:4318/v1/traces")
        );
    }

    #[test]
    fn a_trailing_slash_does_not_produce_a_double_slash() {
        // Trailing slashes are the common form in a Helm value, and `//v1` is a
        // 404 on several ingresses — a silent one, per batch.
        assert_eq!(
            resolve_traces_endpoint(None, Some("http://collector:4318/")).as_deref(),
            Some("http://collector:4318/v1/traces")
        );
    }

    #[test]
    fn the_signal_specific_endpoint_wins_and_is_used_verbatim() {
        // Per the OTLP spec. Appending the signal path to this one instead is
        // the easy mistake, and it fails as a 404 rather than as an error.
        assert_eq!(
            resolve_traces_endpoint(Some("http://other:4318/custom"), Some("http://base:4318"))
                .as_deref(),
            Some("http://other:4318/custom")
        );
    }

    #[test]
    fn no_endpoint_installs_no_exporter() {
        assert_eq!(
            resolve_traces_endpoint(None, None),
            None,
            "absence must be the cheap path: build tools and test binaries must not open a socket"
        );
    }

    #[test]
    fn blank_reads_as_absent_rather_than_as_an_empty_endpoint() {
        assert_eq!(clean(""), None);
        assert_eq!(clean("   "), None);
        assert_eq!(clean(" value "), Some("value".to_string()));
    }

    #[test]
    fn durations_fall_back_instead_of_failing() {
        assert_eq!(parse_secs(Some("30"), 5).as_secs(), 30);
        assert_eq!(parse_secs(None, 5).as_secs(), 5);
        assert_eq!(
            parse_secs(Some("soon"), 5).as_secs(),
            5,
            "a typo in a Helm value must not be a pod that will not boot"
        );
    }
}
