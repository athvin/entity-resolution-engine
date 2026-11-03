//! A record value must not reach an exported span, asserted on the bytes.
//!
//! ## The hole this closes
//!
//! A database error message routinely contains the offending record value.
//! Measured on DuckDB with a real fixture address, not supposed:
//!
//! ```text
//! Constraint Error: Duplicate key "email: robert.chen@example.com"
//! violates primary key constraint.
//! ```
//!
//! Two paths carried that into an exported span, and both shipped before this
//! test existed:
//!
//! 1. `er_span::check` recorded the error's `Display` as `error.type`.
//! 2. `tracing-opentelemetry` turns any event inside a span into a span event,
//!    so `tracing::error!(error = %e)` anywhere in the workspace exported its
//!    fields.
//!
//! Neither is caught by running a clean pipeline and grepping the result —
//! which is exactly what I did, and it passed, because nothing had failed. The
//! leak needs a FAILURE to appear, so the test manufactures one.
//!
//! It asserts on the raw protobuf the collector would receive rather than on
//! the span's fields, because that is the thing that actually leaves the
//! process. A field that is set but filtered before export is harmless; a field
//! that is absent from the span but present in the payload is not, and only the
//! bytes can tell those apart.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use er_obs::Role;
use opentelemetry::trace::TracerProvider as _;
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt;

/// A value of the kind this engine deduplicates: it is the payload, not a label.
const RECORD_VALUE: &str = "robert.chen@example.com";

/// Exactly what DuckDB says on a primary-key violation.
fn constraint_error() -> std::io::Error {
    std::io::Error::other(format!(
        "Constraint Error: Duplicate key \"email: {RECORD_VALUE}\" violates primary key constraint."
    ))
}

/// Accept one export and hand back everything that arrived.
fn one_shot_collector() -> (u16, std::thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binding a loopback listener");
    let port = listener
        .local_addr()
        .expect("the listener has a local address")
        .port();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accepting the export");
        let mut buf = vec![0u8; 256 * 1024];
        let n = stream.read(&mut buf).unwrap_or(0);
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        buf.truncate(n);
        buf
    });
    (port, handle)
}

#[test]
fn a_failing_statement_does_not_export_the_record_value_in_its_error() {
    let (port, collector) = one_shot_collector();
    let provider = er_obs::provider_for(
        Role::Task,
        &format!("http://127.0.0.1:{port}/v1/traces"),
        Duration::from_secs(10),
    )
    .expect("building the provider");

    // The subscriber `init` builds, scoped to this test: `init` installs a
    // process-global one and this crate's tests share a process.
    let subscriber = tracing_subscriber::registry().with(
        tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("pii-probe"))
            .with_filter(er_obs::SpansOnly),
    );

    tracing::subscriber::with_default(subscriber, || {
        let span = er_span::sql_span(
            er_span::Store::Lake,
            "golden_record",
            "update",
            "override_email",
            "golden_records",
            "UPDATE golden_records SET email = $1 WHERE id = $2",
        );
        let _entered = span.enter();

        // Path 1: the error text on the span.
        let failed: Result<(), _> = Err(constraint_error());
        let _ = er_span::check(&span, failed);

        // Path 2: the same text as an event field, which the bridge would
        // otherwise record as a span event.
        tracing::error!(error = %constraint_error(), "stage failed");
    });

    provider.force_flush().expect("flush");
    let payload = collector.join().expect("collector thread");
    let text = String::from_utf8_lossy(&payload);

    assert!(
        !payload.is_empty(),
        "nothing was exported, so this test proves nothing — the span pipeline must be live"
    );
    // The span itself must have arrived; otherwise the absence below is vacuous.
    assert!(
        text.contains("golden_record.update.override_email"),
        "the statement span did not arrive, so the assertions below prove nothing"
    );
    assert!(
        !text.contains(RECORD_VALUE),
        "a record value reached the exported payload — §10.4 forbids values on telemetry, \
         and a constraint violation is how it gets there"
    );
    assert!(
        !text.contains("Duplicate key"),
        "the database error message reached the exported payload"
    );
}

#[test]
fn the_failure_is_still_visible_without_its_message() {
    // Suppressing the text must not suppress the FACT. The collector's `errors`
    // sampling policy keys on the status, and a dashboard counts outcomes — if
    // this regressed to silence, every failed statement would fall to the 10%
    // residual and most would simply vanish.
    let (port, collector) = one_shot_collector();
    let provider = er_obs::provider_for(
        Role::Task,
        &format!("http://127.0.0.1:{port}/v1/traces"),
        Duration::from_secs(10),
    )
    .expect("building the provider");

    let subscriber = tracing_subscriber::registry().with(
        tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("pii-probe"))
            .with_filter(er_obs::SpansOnly),
    );

    tracing::subscriber::with_default(subscriber, || {
        let span = er_span::sql_span(
            er_span::Store::Lake,
            "golden_record",
            "update",
            "override_email",
            "golden_records",
            "UPDATE golden_records SET email = $1",
        );
        let _entered = span.enter();
        let failed: Result<(), _> = Err(constraint_error());
        let _ = er_span::check(&span, failed);
    });

    provider.force_flush().expect("flush");
    let payload = collector.join().expect("collector thread");
    let text = String::from_utf8_lossy(&payload).into_owned();

    // Specifically the STATUS, on the wire. Two earlier versions of this
    // assertion were wrong in opposite directions and both would have passed
    // while the status was missing:
    //
    //   `contains("error")` — satisfied by `er.outcome = error` alone.
    //   `contains("ERROR")` — the status is NOT a string. `tracing-opentelemetry`
    //     intercepts `otel.status_code` as a special field and maps it onto the
    //     span's `Status` message, which OTLP encodes as an enum.
    //
    // So this matches the bytes: field 15 (`status`), length 2, containing
    // field 3 (`code`) = varint 2 = `STATUS_CODE_ERROR`.
    const STATUS_ERROR: &[u8] = &[0x7a, 0x02, 0x18, 0x02];
    assert!(
        payload
            .windows(STATUS_ERROR.len())
            .any(|w| w == STATUS_ERROR),
        "the span must carry Status.code = STATUS_CODE_ERROR. The collector's \
         `errors` tail-sampling policy reads it, and without it a failed \
         statement is indistinguishable from a successful one and falls to the \
         10% residual:\n{text:?}"
    );
    assert!(
        text.contains("er.outcome"),
        "the span must still carry er.outcome"
    );
}
