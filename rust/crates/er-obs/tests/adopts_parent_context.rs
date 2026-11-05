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
use std::time::Duration;

use er_obs::Role;
use opentelemetry::trace::TracerProvider as _;
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt;

/// The W3C level-1 example context, standing in for one the BFF would mint.
const INCOMING: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
const TRACE_ID_HEX: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

fn one_shot_collector() -> (u16, std::thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binding a loopback listener");
    let port = listener
        .local_addr()
        .expect("the listener has a local address")
        .port();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accepting the export");
        let mut buf = vec![0u8; 128 * 1024];
        let n = stream.read(&mut buf).unwrap_or(0);
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        buf.truncate(n);
        buf
    });
    (port, handle)
}

fn trace_id_bytes() -> Vec<u8> {
    (0..TRACE_ID_HEX.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&TRACE_ID_HEX[i..i + 2], 16).expect("hex"))
        .collect()
}

/// Export one span, optionally adopting `INCOMING` first.
fn export_job_span(port: u16, adopt: bool) {
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
        if adopt {
            er_obs::adopt_traceparent(&span, INCOMING);
        }
        let _entered = span.enter();
    });

    provider.force_flush().expect("flush");
    provider.shutdown().ok();
}

#[test]
fn the_job_span_inherits_the_enqueueing_requests_trace_id() {
    let (port, collector) = one_shot_collector();
    export_job_span(port, true);
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
fn without_adoption_the_span_is_a_root() {
    // The control. Without this, the assertion above could pass because the
    // trace id appeared somewhere incidental rather than because adoption
    // worked — and a test that cannot fail for the right reason is not
    // evidence.
    let (port, collector) = one_shot_collector();
    export_job_span(port, false);
    let payload = collector.join().expect("collector thread");

    assert!(!payload.is_empty(), "nothing was exported");
    let needle = trace_id_bytes();
    assert!(
        !payload.windows(needle.len()).any(|w| w == needle),
        "the trace id appeared WITHOUT adoption, so the positive test proves nothing"
    );
}
