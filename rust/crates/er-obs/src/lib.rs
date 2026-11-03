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
//! [`init()`] is the one call an entry point makes. The [`mod@init`] module
//! documents why there is no OTLP exporter in it yet.

pub mod blocking;
pub mod counters;
pub mod init;
pub mod logging;

pub use blocking::spawn_blocking_in_span;
pub use init::{Guard, Role, init};
