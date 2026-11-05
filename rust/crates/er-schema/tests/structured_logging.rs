//! `er-server` logs through `tracing`, not `eprintln!`.
//!
//! ## Why this is a gate and not a convention
//!
//! 114 `eprintln!` calls were converted to make this true. The reason they had
//! to go is correlation, not tidiness: an `eprintln!` carries no trace id, no
//! span context and no level, so a line saying a job failed could not be joined
//! to the request that enqueued it, the run it belonged to, or the SQL
//! statements it executed. In a system whose stated purpose is tracing a
//! session from a click to a statement, that is a hole rather than an
//! inconsistency — and one that reappears the first time somebody debugging at
//! 2am reaches for the shortest thing that prints.
//!
//! A `tracing` event emitted inside a span inherits that span's identifiers for
//! free, which is the whole point.
//!
//! ## Why only er-server
//!
//! The other 53 `eprintln!` calls in the workspace are a **documented human
//! contract** and are deliberately out of scope: `er-entity check`'s diagnostic
//! messages, `er … --json`'s machine-readable output, the S5.2 stage line, and
//! `er-mcp`, whose stdout is JSON-RPC framing a writer would corrupt. Those are
//! sentences a person reads or formats a tool parses, not telemetry.
//!
//! The same reasoning carves out the allowlist below, which is why each entry
//! states its own.

use std::fs;
use std::path::{Path, PathBuf};

/// Call sites that may stay `eprintln!`, each with the reason it is right.
///
/// Keyed by file, with a count, so adding a *new* one to an allowlisted file
/// still fails. A bare filename allowlist would quietly re-open the whole file.
const ALLOWED: &[(&str, usize, &str)] = &[
    (
        "mail.rs",
        1,
        "the `log` mail backend renders a whole email for a developer to read in \
         their terminal — the multi-line subject-and-body layout IS the feature, \
         and it carries a recipient address that must stay off anything exportable",
    ),
    (
        "er-openapi-check.rs",
        5,
        "a CI helper binary, not the server: it prints a usage line and a \
         human-readable list of breaking schema changes for whoever is reading \
         the job log, and it exits before any subscriber exists",
    ),
    (
        "main.rs",
        6,
        "`migrate`, `migrate_tenants` and `install_extensions` are operator CLI \
         verbs whose output a human reads on their terminal; everything in \
         `async_main`, which is the daemon path, is `tracing`",
    ),
];

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

/// Strip `//` comments so prose about `eprintln!` is not read as a call.
///
/// This is not hypothetical: the comment explaining why `mail.rs` keeps its one
/// call contains the word, and an earlier version of this scan counted it. The
/// same trap caught the PII gate in `sql_span_pii.rs`, which is why both strip.
///
/// Line-oriented and therefore imprecise in one direction only: a `//` inside a
/// string literal truncates that line early, which can only cause a **false
/// pass**, never a false failure. Acceptable for a gate whose job is to catch a
/// reintroduction, not to parse Rust.
fn code_only(source: &str) -> String {
    source
        .lines()
        .map(|line| match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn er_server_logs_through_tracing() {
    let server = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../er-server/src"));
    let mut files = Vec::new();
    product_sources(&server, &mut files);

    // A suspicious zero is a failure, not a pass: it is what a moved or renamed
    // source directory looks like, and it would make this gate report green
    // over a tree it never read.
    assert!(
        files.len() > 15,
        "the er-server scan looks wrong ({} files found under {}); this gate must \
         not pass without reading the tree",
        files.len(),
        server.display()
    );

    let mut offenders = Vec::new();
    for file in &files {
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let found = code_only(&fs::read_to_string(file).expect("readable source"))
            .matches("eprintln!")
            .count();
        if found == 0 {
            continue;
        }
        match ALLOWED.iter().find(|(f, _, _)| *f == name) {
            Some((_, budget, _)) if found <= *budget => {}
            Some((_, budget, reason)) => offenders.push(format!(
                "{name}: {found} eprintln! but only {budget} allowed ({reason})"
            )),
            None => offenders.push(format!("{name}: {found} eprintln!")),
        }
    }

    assert!(
        offenders.is_empty(),
        "er-server must log through `tracing`, so an event inherits its span's \
         trace id, run id and org for free. An `eprintln!` carries none of \
         them, so its line cannot be joined to the request that caused it.\n\n{}\n\n\
         Convert it: a constant message, every interpolated value its own field, \
         and an error flattened into an `error` field with `errfmt::chain(&e)` \
         (thiserror) or `{{e:#}}` (anyhow) — never interpolated into the message, \
         which `er-errors/tests/display_no_source.rs` separately forbids.\n\
         If the call is genuinely human-facing output, add it to ALLOWED with \
         the reason it is right.",
        offenders.join("\n")
    );
}

#[test]
fn the_allowlist_is_not_stale() {
    // An allowlist entry whose call sites have gone is worse than none: it keeps
    // a file's budget open for good, and its reason misinforms the next reader.
    // The same failure `deny.toml` documents at length for dead `yanked`
    // exceptions.
    let server = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../er-server/src"));
    let mut files = Vec::new();
    product_sources(&server, &mut files);

    for (name, budget, _) in ALLOWED {
        // Searched in the scanned tree rather than joined onto `src/`, because
        // an allowlisted file need not sit at the top level —
        // `er-openapi-check.rs` lives in `src/bin/`, and a `join` silently
        // looked for it in the wrong place.
        let path = files
            .iter()
            .find(|f| f.file_name().and_then(|n| n.to_str()) == Some(*name))
            .unwrap_or_else(|| {
                panic!("ALLOWED names {name}, which no longer exists under er-server/src")
            });
        let found = code_only(&fs::read_to_string(path).expect("readable source"))
            .matches("eprintln!")
            .count();
        assert_eq!(
            found, *budget,
            "{name} is allowed {budget} eprintln! but has {found}. If the count \
             dropped, lower or remove the ALLOWED entry — a budget larger than \
             reality silently permits a reintroduction."
        );
    }
}
