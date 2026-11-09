//! Observability: installing the subscriber, plus the S5.2 counter vocabulary
//! and structured stage log line (ports of `src/er/obs/counters.py` and
//! `src/er/obs/logging.py`).
//!
//! This is the half of the telemetry layer that **collects**, so only binaries
//! depend on it. Library crates emit through `er-span`, which carries nothing
//! but `tracing` — the split `tracing` is designed around, and the reason
//! adding telemetry here cannot inflate the dependency graph of `er`, `dedupe`
//! or every test binary.
//!
//! [`init()`] is the one call an entry point makes. It always installs the
//! structured stderr layer and additionally exports spans over OTLP when an
//! endpoint is configured; [`mod@otlp`] carries the reasoning for how that
//! exporter is built, which is less obvious than it looks.

pub mod blocking;
pub mod counters;
pub mod executions;
pub mod identity;
pub mod init;
pub mod logging;
pub mod metrics;
pub mod otlp;
pub mod propagate;
pub mod registry;
// The span-capture harness, for crates that must assert a span's lineage
// without declaring the OTel versions themselves. Feature-gated so it is not
// part of the shipped library — see the module doc for why it is here at all.
#[cfg(feature = "test-support")]
pub mod testing;

pub use blocking::spawn_blocking_in_span;
pub use executions::{Execution, ExecutionRecorder};
pub use identity::current_trace_id;
pub use init::{Guard, Role, SpansOnly, init};
pub use otlp::{ReqwestClient, flush_timeout, metrics_endpoint, provider_for, traces_endpoint};
pub use propagate::adopt_traceparent;
