//! A job span inherits the trace id of the request that enqueued it.
//!
//! This is the assertion that distinguishes "one trace" from "two traces and a
//! string that joins them". Recording `er.trace_context` as a field — which the
//! dispatcher did for several PRs — makes a log search possible and a trace
//! tree impossible. The only way to tell those apart is to look at the
//! exported `trace_id`, which is what this does.
//!
//! The trace id is asserted on the wire as its sixteen raw bytes, because that
//! is how OTLP encodes it. Asserting on a hex string would pass while the
//! actual field held something else entirely, which is a mistake already made
//! once in this crate's tests against `otel.status_code`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use er_obs::Role;
use opentelemetry::trace::TracerProvider as _;
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt;

/// The W3C level-1 example context, standing in for one the BFF would mint.
const INCOMING: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
/// The same context with the sampled flag **clear**, which is a valid
/// traceparent that `er_server::acceptable_traceparent` accepts and stores.
const INCOMING_UNSAMPLED: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00";
const TRACE_ID_HEX: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

/// How long the collector waits for an export before giving up and reporting
/// an empty payload.
///
/// **It must give up rather than block.** A regression that stops spans being
/// exported at all — removing `provider_for`'s sampler is exactly one — means
/// nothing ever connects, and a bare `accept()` then waits forever. In this
/// repository that surfaces as a CI job TIMEOUT, which reads as infrastructure
/// flakiness rather than as the assertion failure it is. Returning empty lets
/// `assert!(!payload.is_empty(), …)` say what actually happened.
const COLLECT_TIMEOUT: Duration = Duration::from_secs(10);

fn one_shot_collector() -> (u16, std::thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binding a loopback listener");
    let port = listener
        .local_addr()
        .expect("the listener has a local address")
        .port();
    listener
        .set_nonblocking(true)
        .expect("the listener accepts a nonblocking mode");
    let handle = std::thread::spawn(move || {
        let deadline = Instant::now() + COLLECT_TIMEOUT;
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_nonblocking(false)
                        .expect("the accepted stream returns to blocking mode");
                    stream
                        .set_read_timeout(Some(COLLECT_TIMEOUT))
                        .expect("the accepted stream takes a read timeout");
                    let mut buf = vec![0u8; 128 * 1024];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
                    buf.truncate(n);
                    return buf;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Vec::new();
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => panic!("accepting the export: {e}"),
            }
        }
    });
    (port, handle)
}

fn trace_id_bytes() -> Vec<u8> {
    (0..TRACE_ID_HEX.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&TRACE_ID_HEX[i..i + 2], 16).expect("hex"))
        .collect()
}

/// Export one span, adopting `traceparent` first when one is given.
fn export_job_span(port: u16, adopt: Option<&str>) {
    let provider = er_obs::provider_for(
        Role::Dispatcher,
        &format!("http://127.0.0.1:{port}/v1/traces"),
        Duration::from_secs(10),
    )
    .expect("building the provider");

    let subscriber = tracing_subscriber::registry().with(
        tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("parent-probe"))
            .with_filter(er_obs::SpansOnly),
    );

    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!("job", er.job_id = "01JOB");
        if let Some(traceparent) = adopt {
            er_obs::adopt_traceparent(&span, traceparent);
        }
        let _entered = span.enter();
    });

    provider.force_flush().expect("flush");
    provider.shutdown().ok();
}

#[test]
fn the_job_span_inherits_the_enqueueing_requests_trace_id() {
    let (port, collector) = one_shot_collector();
    export_job_span(port, Some(INCOMING));
    let payload = collector.join().expect("collector thread");

    assert!(!payload.is_empty(), "nothing was exported");
    let needle = trace_id_bytes();
    assert!(
        payload.windows(needle.len()).any(|w| w == needle),
        "the job span did not inherit the incoming trace id — it is a root, so a \
         click and the job it caused are two traces joined by a string rather \
         than one trace"
    );
}

#[test]
fn an_unsampled_remote_parent_does_not_silence_our_spans() {
    // `provider_for` states `Sampler::AlwaysOn` rather than taking the
    // builder's default, and this is the assertion that makes that line
    // load-bearing instead of decorative.
    //
    // The default is `ParentBased(AlwaysOn)`, which honours a REMOTE parent's
    // sampled flag. That was harmless while nothing adopted a remote parent.
    // Now that both the dispatcher and the HTTP request span adopt one, the
    // default would hand any caller a kill switch on this process's telemetry:
    // send `…-00` and our spans stop being recorded. The flag is the untrusted
    // side of a header that is spoofable by design, and the silencing would be
    // invisible — spans would simply stop arriving for that caller, with
    // nothing logged.
    //
    // Measured both ways while this was written: with the sampler stated, one
    // span; with the line removed, zero.
    let (port, collector) = one_shot_collector();
    export_job_span(port, Some(INCOMING_UNSAMPLED));
    let payload = collector.join().expect("collector thread");

    assert!(
        !payload.is_empty(),
        "an unsampled remote parent silenced the span entirely — a caller can \
         now switch off this process's telemetry with one header"
    );
    let needle = trace_id_bytes();
    assert!(
        payload.windows(needle.len()).any(|w| w == needle),
        "the span exported but did not join the caller's trace"
    );
}

#[test]
fn without_adoption_the_span_is_a_root() {
    // The control. Without this, the assertion above could pass because the
    // trace id appeared somewhere incidental rather than because adoption
    // worked — and a test that cannot fail for the right reason is not
    // evidence.
    let (port, collector) = one_shot_collector();
    export_job_span(port, None);
    let payload = collector.join().expect("collector thread");

    assert!(!payload.is_empty(), "nothing was exported");
    let needle = trace_id_bytes();
    assert!(
        !payload.windows(needle.len()).any(|w| w == needle),
        "the trace id appeared WITHOUT adoption, so the positive test proves nothing"
    );
}
