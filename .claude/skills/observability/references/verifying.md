# Verifying a producer

## Contents

- [A clean run cannot find the defects that matter](#a-clean-run-cannot-find-the-defects-that-matter)
- [Proving the producer from a laptop](#proving-the-producer-from-a-laptop)
- [The traps that make a run lie](#the-traps-that-make-a-run-lie)
- [Assert on the exported bytes](#assert-on-the-exported-bytes)

## A clean run cannot find the defects that matter

A successful pipeline exercises none of the error paths, so a PII probe over it
passes for the wrong reason — nothing failed, so no error message was ever
formatted. The two worst span defects this layer has had were both found only by
**manufacturing a failure**: the undeclared-field no-op, and events exporting a
database error that carried a record value.

So when verifying instrumentation, make something fail on purpose. A primary-key
violation is the cheap one — it is what produces a message with a real attribute
value in it.

The same reasoning applies to a guard: a precondition that always holds makes the
guard decoration. Mutate the condition and confirm the gate notices.

## Proving the producer from a laptop

No image build and no cluster deploy are needed to prove spans leave the
process and arrive intact.

1. Port-forward the **span store**, not the collector:

   ```sh
   kubectl -n observability port-forward svc/openobserve 25080:5080
   ```

   The collector chart binds its OTLP receiver to `${MY_POD_IP}`, so a
   port-forward to localhost gets connection-refused — which looks like a broken
   exporter.

2. Point the exporter at it, with the ingest credential from the
   `openobserve-ingest` secret:

   ```sh
   export OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://localhost:25080/api/default/v1/traces
   export OTEL_EXPORTER_OTLP_TRACES_HEADERS="Authorization=$AUTH"
   ```

   The signal-specific variable is used **verbatim**; the general
   `OTEL_EXPORTER_OTLP_ENDPOINT` is a *base* that the signal path is appended
   to. Getting that backwards is a silent 404 per batch.

3. Run the binary against the CLI's own fixtures, then query the store's search
   endpoint for the trace.

## The traps that make a run lie

**Leave `RUST_LOG` unset.** A narrow directive like `er_obs=debug,er_cli=info`
has no global level, so the crate where the generated statement spans live is off
entirely and only the run and stage spans arrive. `DEFAULT_FILTER`'s global
`info` is what lets them through. A filter also drops a span from the *ancestry*
when it excludes it by target, so a narrow filter loses the process span's
service identity from every record beneath it.

**A blank environment variable reads as absent, on purpose.** An unset Helm value
renders as `""` rather than vanishing, so an empty endpoint must not be treated
as configured.

**Nothing arriving has three distinct causes** — no endpoint configured, an
exporter that failed to build, and an exporter that built and shipped elsewhere.
They are distinguishable only because the build outcome is reported *after* the
subscriber is installed; a diagnostic emitted before that goes to a subscriber
that does not exist yet and is discarded.

**A panic on the exporter's own thread does not fail the process.** The SDK's
batch processor owns a plain `std::thread` and polls the export future there with
a non-tokio executor, so an async HTTP client without a runtime panics on that
thread while `main` runs on and every span is dropped. The failure mode is a
telemetry pipeline that looks installed and exports nothing, and it is invisible
to any test that only builds a provider. Half this workspace's roles — the
pipeline CLI and the pod verb — have no runtime at all, which is why the client
owns one.

**Overhead is not measurable on a laptop.** A debug-build baseline swings wide
enough to swamp the signal. What bounds it is arithmetic — spans per run times a
plausible per-span cost — not an A/B from this environment. Do not quote one.

## Assert on the exported bytes

A field set but filtered before export is harmless; a field absent from the span
but present in the payload is not. So a test that matters reads what a real
listener received, not what the span claims to carry.

One concrete consequence: `otel.status_code` is **not a string on the wire**.
`tracing-opentelemetry` maps it onto the protobuf `Span.status`, so a payload
assertion looking for the text `ERROR` passes while the status is missing.
