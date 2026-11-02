//! The ONE test binary for this crate. Every file in `tests/` is a module of it
//! rather than a target of its own, because sccache cannot cache a final link
//! and each extra target is an uncached link on every CI run. See
//! `dedupe-core/tests/main.rs`, which explains the same arrangement at length.

mod fixtures;
mod reference_doc;
