//! The posterity gate for blocking hops: no bare `spawn_blocking` in the
//! control plane.
//!
//! `tracing`'s current span is thread-local, and `tokio::task::spawn_blocking`
//! moves its closure to another thread. The bare form therefore drops the
//! ambient span, and everything the closure creates becomes a disconnected
//! root. In this workspace that is almost always the whole engine: DuckDB is
//! synchronous, so the pipeline runs inside closures the async dispatcher hands
//! off, and a single missed hop detaches every stage span and every statement
//! span beneath it from the job that asked for them.
//!
//! **Nothing else catches this.** No test fails, no type is wrong, every span
//! is still produced — they are simply orphans. It does not read as a bug; it
//! reads as tracing not working. So the rule is structural rather than a
//! convention, in the shape `freeform_sql.rs` established.
//!
//! `er_obs::spawn_blocking_in_span` captures `Span::current()` on the calling
//! thread and enters it inside the closure. It is otherwise identical — same
//! signature, same `JoinHandle`, same panic-as-`JoinError` contract.

use std::fs;
use std::path::{Path, PathBuf};

/// Every `.rs` file under `dir`, skipping `target/`, dotted dirs, `tests/` and
/// `benches/`. Tests may spawn freely: an orphaned span in a test harness
/// costs nothing, and forcing the wrapper there would only add noise.
fn product_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("readable workspace dir") {
        let path = entry.expect("dir entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if name == "target" || name == "tests" || name == "benches" || name.starts_with('.') {
                continue;
            }
            product_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// The token a call site uses. Doc comments say `spawn_blocking` without the
/// path, so matching the qualified call avoids flagging prose — and the
/// workspace has no `use tokio::task::spawn_blocking` import anywhere, which is
/// what makes the qualified form the only spelling to look for.
const BARE_SPAWN: &str = "tokio::task::spawn_blocking(";

#[test]
fn the_control_plane_carries_its_span_across_every_blocking_hop() {
    let crates = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
    let mut files = Vec::new();
    product_sources(&crates.join("er-server/src"), &mut files);

    // A zero here would mean the scan examined nothing — a directory rename
    // would otherwise report success having looked at no files at all, which is
    // the failure mode that let a gate in this repo pass while blind.
    assert!(
        files.len() > 10,
        "expected er-server/src to hold many sources, found {} — did the layout move?",
        files.len()
    );

    let mut offenders = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file).expect("readable source");
        for (i, line) in text.lines().enumerate() {
            if line.contains(BARE_SPAWN) {
                offenders.push(format!(
                    "{}:{}",
                    file.strip_prefix(&crates).unwrap_or(file).display(),
                    i + 1
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "a blocking hop drops the ambient span, so every span the closure makes \
         becomes a disconnected root — and nothing else fails when it does. Use \
         `er_obs::spawn_blocking_in_span` instead of `tokio::task::spawn_blocking`:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn the_wrapper_is_actually_used() {
    // The converse of the scan above: proving the bare form is absent is only
    // meaningful if the hops still exist. If this drops to zero, either the
    // dispatcher stopped handing work to blocking threads (a real change worth
    // noticing) or someone imported the bare form under another name.
    let crates = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
    let mut files = Vec::new();
    product_sources(&crates.join("er-server/src"), &mut files);

    let used = files
        .iter()
        .filter_map(|f| fs::read_to_string(f).ok())
        .map(|t| t.matches("spawn_blocking_in_span(").count())
        .sum::<usize>();

    assert!(
        used >= 9,
        "expected the span-carrying wrapper at every blocking hop in er-server, \
         found {used} uses — if a hop was removed, lower this floor deliberately"
    );
}
