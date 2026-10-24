//! The posterity gate for the workspace boundary: no source under `rust/` may
//! `include_str!` / `include_bytes!` / `include!` a path that escapes the cargo
//! workspace root.
//!
//! The cargo workspace root is `rust/`, not the repo root — `CLAUDE.md` says so
//! and the Makefile's `cd rust` encodes it. `entities/` sits *beside* the
//! workspace at the repo root, so a generated module that embedded its YAML as
//!
//! ```ignore
//! include_str!("../../../../entities/webhook/entity.yaml")
//! ```
//!
//! compiled fine from a local `cargo build` (which runs with the whole repo on
//! disk) and then failed every build whose context is the workspace. That is
//! exactly what happened: the image build sets `context: rust`, so `entities/`
//! was simply not there, and `er-entities` failed with one error per entity —
//! breaking every deploy while `make check` stayed green.
//!
//! An embedded-asset path is therefore only legal if it stays inside `rust/`.
//! `er-entity-gen` now inlines the entity YAML as raw string literals instead,
//! which is what the generated catalog module's own header always claimed
//! ("without YAML files on disk").
//!
//! **Scope: what the image actually compiles.** The image runs
//! `cargo build --release --bin …`, so only non-test code has to be
//! self-contained. Reaching into `fixtures/` or `frontend/` from a test is
//! fine and several tests do it on purpose — `er-server`'s OpenAPI drift check
//! embeds the committed `openapi.json`, and `mock_contract.rs` embeds the
//! frontend's mock server. Those are the gate's reason for skipping `tests/`,
//! `benches/` and `#[cfg(test)]` modules, not an oversight.

use std::fs;
use std::path::{Path, PathBuf};

/// Every `.rs` file under `dir` that ships in a binary: `target/`, dotted
/// directories, `tests/` and `benches/` are skipped, as is this gate itself
/// (its own prose quotes the offending pattern).
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

/// The byte ranges covered by `#[cfg(test)]` items — brace-matched from the
/// attribute to the end of the item it guards.
///
/// Fails *closed*: if the braces do not balance the range is dropped, so the
/// gate keeps scanning rather than silently excusing the file.
fn cfg_test_ranges(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    for (start, _) in text.match_indices("#[cfg(test)]") {
        let Some(open) = bytes[start..]
            .iter()
            .position(|&b| b == b'{')
            .map(|i| start + i)
        else {
            continue;
        };
        let mut depth = 0usize;
        let mut end = None;
        for (i, &b) in bytes.iter().enumerate().skip(open) {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        if let Some(end) = end {
            ranges.push((start, end));
        }
    }
    ranges
}

/// Does `literal`, resolved relative to the directory holding `file`, land
/// outside `root`? Purely lexical: `..` pops a segment, `.` and empty segments
/// are skipped, and a path that pops past `root` is the violation. No
/// filesystem access, so a missing file is still judged on its path.
fn escapes_root(file: &Path, literal: &str, root: &Path) -> bool {
    // An absolute path is always an escape — it cannot be inside a relocatable
    // workspace in any meaningful sense.
    if literal.starts_with('/') {
        return true;
    }
    let Ok(rel_dir) = file
        .parent()
        .unwrap_or(Path::new(""))
        .strip_prefix(root)
        .map(Path::to_path_buf)
    else {
        return false;
    };
    let mut depth: i32 = rel_dir.components().count() as i32;
    for segment in literal.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            _ => depth += 1,
        }
    }
    false
}

/// The string literal argument of the macro invocation starting at `from`, or
/// `None` when the argument is not a plain literal (e.g. `concat!(…)`), which
/// this gate does not try to evaluate.
fn literal_arg(text: &str, from: usize) -> Option<&str> {
    let open = text[from..].find('(')? + from + 1;
    let rest = text[open..].trim_start();
    let quote = rest.strip_prefix('"')?;
    let end = quote.find('"')?;
    Some(&quote[..end])
}

#[test]
fn no_embedded_asset_escapes_the_cargo_workspace() {
    // CARGO_MANIFEST_DIR is rust/crates/er-entity-gen; the workspace root is
    // two levels up.
    let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .canonicalize()
        .expect("workspace root resolves");
    let mut files = Vec::new();
    product_sources(&root, &mut files);
    assert!(
        files.len() > 60,
        "the workspace scan looks wrong ({} files found under {})",
        files.len(),
        root.display()
    );

    let mut offenders: Vec<String> = Vec::new();
    let mut includes_seen = 0usize;

    for file in &files {
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        let skip = cfg_test_ranges(&text);
        for macro_name in ["include_str!", "include_bytes!", "include!"] {
            for (idx, _) in text.match_indices(macro_name) {
                if skip.iter().any(|&(lo, hi)| idx > lo && idx < hi) {
                    continue; // inside a #[cfg(test)] item: never in a binary.
                }
                let Some(literal) = literal_arg(&text, idx + macro_name.len() - 1) else {
                    continue; // concat!(env!(…), …) and friends: out of scope.
                };
                includes_seen += 1;
                if escapes_root(file, literal, &root) {
                    let line = text[..idx].lines().count();
                    offenders.push(format!(
                        "{}:{line}: {macro_name}(\"{literal}\") leaves the workspace",
                        file.strip_prefix(&root).unwrap_or(file).display(),
                    ));
                }
            }
        }
    }

    // Self-check: the scan must still be finding embedded assets at all. The
    // shipping ones are er-entity-gen's three `assets/` files (batch.rs,
    // pg_ddl_log.sql, pg_history.sql); everything else that embeds a file does
    // it from a test.
    assert!(
        includes_seen >= 3,
        "found only {includes_seen} literal include macros outside #[cfg(test)] — \
         the scan is not finding them"
    );
    assert!(
        offenders.is_empty(),
        "embedded assets must live inside the cargo workspace root ({}), because a \
         build whose context is the workspace (the container image) has nothing \
         above it:\n  {}",
        root.display(),
        offenders.join("\n  ")
    );
}

#[test]
fn escapes_root_counts_segments_lexically() {
    let root = Path::new("/w");
    let file = Path::new("/w/crates/a/src/lib.rs");
    // src -> a -> crates -> /w: four pops reach the root's parent.
    assert!(escapes_root(file, "../../../../entities/x.yaml", root));
    // Three pops land on the workspace root itself, which is still inside it.
    assert!(!escapes_root(file, "../../../entities/x.yaml", root));
    assert!(!escapes_root(file, "../assets/batch.rs", root));
    assert!(!escapes_root(file, "fixtures/x.json", root));
    // A descent before the pops pays for them.
    assert!(!escapes_root(file, "deep/../../../../x", root));
    assert!(escapes_root(file, "/etc/passwd", root));
}
