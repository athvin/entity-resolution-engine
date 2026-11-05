//! The ONE test binary for this crate: every file in `tests/` is a module of it
//! rather than a target of its own.
//!
//! sccache cannot cache a final link — it reports these as
//! `Non-cacheable reasons: crate-type` — so each test target was compiled and
//! linked uncached on every run of every CI job. `autotests = false` in
//! Cargo.toml stops cargo re-discovering each file as a target; the files keep
//! their paths, so `include_str!` and `CARGO_MANIFEST_DIR`-relative fixtures
//! still resolve. See er-server/tests/main.rs for the full reasoning.
//!
//! To run one file's tests: `cargo test -p er-obs --test main <name>`.

mod adopts_parent_context;
mod cardinality;
mod no_pii_in_exported_spans;
mod otlp_export;
mod reference;
