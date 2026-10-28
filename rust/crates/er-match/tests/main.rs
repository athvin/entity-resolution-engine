//! The ONE test binary for this crate. Every file in `tests/` is a module of it
//! rather than a target of its own.
//!
//! sccache cannot cache a final link (it reports these as
//! `Non-cacheable reasons: crate-type`), so every test target is compiled and
//! linked uncached on each run of each CI job, and each one statically links
//! bundled DuckDB. Collapsing them is the only thing that moves that floor.
//!
//! As modules of one binary these run as threads in one process instead of
//! binary-at-a-time, which is where the wall-clock win comes from. Safe here
//! because the golden fixtures are read-only committed files.
//!
//! `autotests = false` in Cargo.toml stops cargo re-discovering each file as a
//! target. Files keep their paths, so `include_str!` and the
//! `CARGO_MANIFEST_DIR`-relative fixture helpers still resolve.
//!
//! To run one file's tests: `cargo test -p er-match --test main <name>`.

mod common;

mod batch_scenarios;
mod dump_corpus;
mod events;
mod golden;
mod match_scores;
mod membership;
mod model;
