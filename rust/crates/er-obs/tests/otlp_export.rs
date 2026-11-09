//! A span, over a real socket, in both of this workspace's runtime shapes.
//!
//! ## Why this is an integration test and not a unit test
//!
//! The SDK's batch processor owns a plain `std::thread` and polls the exporter
//! future on it with a non-tokio executor. An async `reqwest::Client` needs a
//! tokio reactor, so a naive implementation panics there:
//!
//! ```text
//! thread 'OpenTelemetry.Traces.BatchProcessor' panicked:
//! there is no reactor running, must be called from the context of a Tokio 1.x runtime
//! ```
//!
//! **The process survives that panic.** It happens on the processor's own
//! thread, so `main` runs on, `force_flush` returns an error nobody is obliged
//! to read, and every span is dropped. Observed exactly once, while developing
//! this exporter, and it is the reason these tests assert on **bytes received
//! by a listener** rather than on a provider that merely built. Constructing a
//! provider proves nothing: the first version of this exporter built fine,
//! exported nothing, and said `Ok` to everything except the flush.
//!
//! ## Why the export case is covered twice
//!
//! The roles genuinely differ and one implementation has to serve both.
//! `er-server`'s API and dispatcher arms are `#[tokio::main]`; the `er` pipeline
//! CLI and the `exec-task` pod verb deliberately are not, because DuckDB is
//! synchronous and `er-server/src/main.rs` records that entering a runtime would
//! put blocking work on a reactor thread. A fix that serves one shape and not
//! the other is the plausible regression, and only the pair catches it.
//!
//! One guess about *which* regression was wrong, and it is worth recording
//! because it reads as obvious: `block_on` in place of `spawn` was expected to
//! pass the no-runtime case and panic the in-runtime one. It passes **both** —
//! the SDK polls `send_bytes` on the batch processor's own thread either way, so
//! there is no ambient runtime to conflict with. These tests therefore do not
//! discriminate between those two implementations, and the module they test says
//! so rather than implying a guarantee they do not provide.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use er_obs::Role;
use opentelemetry::trace::{Tracer, TracerProvider};

/// What a one-shot collector saw.
struct Received {
    bytes: usize,
    head: String,
    body: Vec<u8>,
}

/// Accept exactly one request, answer 200, and report what arrived.
///
/// Bound on port 0 so the two tests in this binary cannot collide when cargo
/// runs them concurrently.
fn one_shot_collector() -> (u16, std::thread::JoinHandle<Received>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binding a loopback listener");
    let port = listener
        .local_addr()
        .expect("the listener has a local address")
        .port();

    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accepting the export");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut data = Vec::new();
        let header_end = loop {
            let mut chunk = [0u8; 4096];
            let n = stream.read(&mut chunk).expect("reading OTLP request");
            assert!(n > 0, "request ended before its headers");
            data.extend_from_slice(&chunk[..n]);
            if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
            assert!(data.len() < 64 * 1024, "unexpectedly large headers");
        };
        let headers = String::from_utf8_lossy(&data[..header_end]);
        let length: usize = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap())
            })
            .expect("content length");
        let mut body = data[header_end..].to_vec();
        let present = body.len();
        assert!(present <= length);
        body.resize(length, 0);
        stream
            .read_exact(&mut body[present..])
            .expect("complete OTLP payload");
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        let _ = stream.flush();
        Received {
            bytes: header_end + body.len(),
            head: format!("{}{}", headers, String::from_utf8_lossy(&body)),
            body,
        }
    });

    (port, handle)
}

/// Emit one span through a provider pointed at `port`, then flush.
fn export_one_span(port: u16) {
    let provider = er_obs::provider_for(
        Role::Task,
        &format!("http://127.0.0.1:{port}/v1/traces"),
        Duration::from_secs(10),
    )
    .expect("building the provider");

    provider
        .tracer("otlp_export_test")
        .in_span("a-real-span", |_| {});

    provider
        .force_flush()
        .expect("force_flush must deliver the batch, not merely return");
    let _ = provider.shutdown();
}

fn assert_it_looked_like_otlp(received: &Received) {
    assert!(
        received.bytes > 0,
        "the collector accepted a connection but received no bytes — \
         the exporter built and exported nothing"
    );
    assert!(
        received.head.starts_with("POST /v1/traces "),
        "the signal path must be joined onto the endpoint exactly once; got: {:?}",
        received.head.lines().next()
    );
    assert!(
        received
            .head
            .to_ascii_lowercase()
            .contains("content-type: application/x-protobuf"),
        "http-proto must post protobuf, not JSON — a wrong content-type is a \
         415 the exporter reports only in its own logs"
    );
    // The resource is what attributes a span to its service, and the OTLP
    // transform takes it from what the exporter was TOLD via `set_resource`,
    // not from each span — so a wrapper that swallows that call ships spans
    // with an empty resource: bytes arrive, nothing is attributable, and the
    // span store reads exactly like the #941 outage. Protobuf embeds the
    // string verbatim, so the service name is findable in the raw payload.
    assert!(
        received.head.contains("er-task"),
        "the payload must carry service.name — a span that arrives \
         unattributed is the outage this file exists to prevent; got: {:?}",
        received.head
    );
}

/// The CLI and pod-verb shape: no ambient tokio runtime anywhere.
///
/// This is the case that panicked on the batch processor's thread, so if the
/// exporter regresses to using the caller's runtime, this test is what fails.
#[test]
fn exports_a_span_with_no_ambient_tokio_runtime() {
    let (port, collector) = one_shot_collector();
    export_one_span(port);
    let received = collector.join().expect("the collector thread panicked");
    assert_it_looked_like_otlp(&received);
}

/// The API and dispatcher shape: called from inside a multi-threaded runtime.
///
/// Called directly on a runtime worker, because that is where it really happens:
/// `Guard::force_flush` runs on every exit path, and in `er-server` those paths
/// are inside `#[tokio::main]`. The blocking flush therefore occupies a worker
/// while the export itself proceeds on the exporter's own runtime — if those two
/// were ever the same runtime, this test is what would hang.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exports_a_span_from_inside_a_tokio_runtime() {
    let (port, collector) = one_shot_collector();
    export_one_span(port);
    let received = collector.join().expect("the collector thread panicked");
    assert_it_looked_like_otlp(&received);
}

/// With no endpoint configured, nothing is installed and nothing is opened.
///
/// The cheap path matters more than it looks: `er-entity` and `dedupe` are build
/// tools and every test binary in the workspace calls `init`, so an exporter
/// that defaults to on would have each of them trying to reach a collector.
#[test]
fn no_endpoint_configured_means_no_exporter() {
    // CI sets neither variable. Asserting on the resolver rather than on `init`
    // keeps this independent of whichever test installed the global subscriber
    // first — `init` is `OnceLock`-guarded and the modules here share a process.
    if std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_none()
        && std::env::var_os("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").is_none()
    {
        assert_eq!(er_obs::traces_endpoint(), None);
    }
}

/// A first rare failure must reach awsemf as an interval count, not become its
/// cumulative baseline. Decode real protobuf, including the histogram's interval
/// statistics and a gauge that must remain a current value.
#[test]
fn first_metric_export_contains_native_deltas() {
    use opentelemetry::KeyValue;
    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
    use opentelemetry_proto::tonic::metrics::v1::{
        AggregationTemporality, metric, number_data_point,
    };
    use prost::Message as _;

    let (port, collector) = one_shot_collector();
    let provider = er_obs::otlp::meter_provider_for(
        Role::Dispatcher,
        &format!("http://127.0.0.1:{port}/v1/metrics"),
        Duration::from_secs(10),
    )
    .unwrap();
    let meter = provider.meter("er-obs");
    meter.u64_counter("er.pods.failures").build().add(
        1,
        &[
            KeyValue::new("resource_class", "S"),
            KeyValue::new("reason", "oom"),
        ],
    );
    meter
        .f64_histogram("er.http.duration")
        .build()
        .record(25.0, &[]);
    meter
        .u64_gauge("er.jobs.queue.total")
        .build()
        .record(7, &[]);
    provider.force_flush().unwrap();
    let received = collector.join().unwrap();
    let _ = provider.shutdown();
    assert!(received.head.starts_with("POST /v1/metrics "));
    let request = ExportMetricsServiceRequest::decode(received.body.as_slice()).unwrap();
    let metrics: Vec<_> = request
        .resource_metrics
        .into_iter()
        .flat_map(|r| r.scope_metrics)
        .flat_map(|s| s.metrics)
        .collect();
    assert_eq!(metrics.len(), 3);
    for metric in metrics {
        match metric.data.unwrap() {
            metric::Data::Sum(sum) => {
                assert_eq!(metric.name, "er.pods.failures");
                assert_eq!(
                    sum.aggregation_temporality,
                    AggregationTemporality::Delta as i32
                );
                assert!(sum.is_monotonic);
                assert_eq!(
                    sum.data_points[0].value,
                    Some(number_data_point::Value::AsInt(1))
                );
            }
            metric::Data::Histogram(histogram) => {
                assert_eq!(metric.name, "er.http.duration");
                assert_eq!(
                    histogram.aggregation_temporality,
                    AggregationTemporality::Delta as i32
                );
                assert_eq!(histogram.data_points[0].count, 1);
                assert_eq!(histogram.data_points[0].sum, Some(25.0));
            }
            metric::Data::Gauge(gauge) => {
                assert_eq!(metric.name, "er.jobs.queue.total");
                assert_eq!(
                    gauge.data_points[0].value,
                    Some(number_data_point::Value::AsInt(7))
                );
            }
            other => panic!("unexpected metric: {other:?}"),
        }
    }
}
