//! The posterity gate for free-form lake SQL: after the reads migration (F0–F6)
//! every persistent lake statement — read or write — is a declared entity
//! statement, a plan-vector-tested generator, or the engine core's own SQL.
//! These scans make regression structural:
//!
//! 1. `Session::query_strings` has no callers outside the engine core
//!    (dedupe-core/dedupe-cli) and tests. The one product exemption — er-cli's
//!    preview reads over its `__preview_src` session temp — is visible in the
//!    call itself (`query_scratch_strings`), not in an allowlist here.
//! 2. `er-server/src` names no `lake.main.<relation>` at all: its reads come
//!    only from `er_entities::` statements and the er-golden merge-plan
//!    generator. No query string is assembled at runtime anywhere in er-server.
//!
//! The dynamic halves are the DuckLake e2e tiers (only-declared relations; the
//! base_10 goldens) — this is the static half, beside `declared_names.rs`.

use std::fs;
use std::path::{Path, PathBuf};

/// Every `.rs` file under `dir`, skipping `target/`, dotted dirs, `tests/` and
/// `benches/` (test SQL probes are scratch by definition).
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

#[test]
fn query_strings_has_no_callers_outside_the_engine_core() {
    let crates = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
    let mut files = Vec::new();
    product_sources(&crates, &mut files);
    assert!(
        files.len() > 60,
        "the workspace scan looks wrong ({} files found under {})",
        files.len(),
        crates.display()
    );

    let mut offenders: Vec<String> = Vec::new();
    for file in &files {
        let exempt = file.components().any(|c| {
            let n = c.as_os_str();
            n == "dedupe-core" || n == "dedupe-cli"
        });
        if exempt {
            continue;
        }
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        if text.contains(".query_strings(") {
            offenders.push(file.display().to_string());
        }
    }
    assert!(
        offenders.is_empty(),
        "free-form lake reads outside the engine core — declare a statement \
         (entities/<entity>/read/) or, for a session-temp scratch read, use \
         Session::query_scratch_strings:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn er_server_builds_no_lake_sql() {
    let server_src = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../er-server/src"));
    let mut files = Vec::new();
    product_sources(&server_src, &mut files);
    assert!(
        files.len() >= 5,
        "the er-server scan looks wrong ({} files)",
        files.len()
    );

    const NEEDLE: &str = "lake.main.";
    let mut offenders: Vec<String> = Vec::new();
    for file in &files {
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        // A qualified relation literal is NEEDLE followed by a name character;
        // prose like `lake.main.*` and the generator's bare `"lake.main"`
        // qualifier do not match.
        let mut rest = text.as_str();
        while let Some(i) = rest.find(NEEDLE) {
            rest = &rest[i + NEEDLE.len()..];
            if rest
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                offenders.push(file.display().to_string());
                break;
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "er-server names a lake relation in source — its lake access goes \
         through er_entities:: statements or the er-golden merge-plan generator, \
         never hand-assembled SQL:\n{}",
        offenders.join("\n")
    );
}
