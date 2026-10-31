//! A control task runs inside the dispatcher with the control plane's own
//! credentials, so it must be STRUCTURALLY unable to run a stage: no file
//! under `src/dags/control/` may name `er_cli::`, `dedupe_core::` or
//! `open_lake` (batch-orchestration-design §3.3/§3.6). The same textual gate
//! shape as `egress_guard.rs` and `dispatch_coverage.rs` — scan the source,
//! self-check the scanner, fail loudly on an empty walk.

use std::fs;
use std::path::PathBuf;

const NEEDLES: [&str; 3] = ["er_cli::", "dedupe_core::", "open_lake"];

fn control_sources() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/dags/control");
    let mut out = Vec::new();
    let entries = fs::read_dir(&dir).expect("src/dags/control exists");
    for entry in entries {
        let path = entry.expect("a readable entry").path();
        if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
}

/// Lines that are code: a line whose trimmed form starts with `//` is a
/// comment, which may legitimately NAME the forbidden symbols (this gate's own
/// documentation does).
fn code_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
}

#[test]
fn no_control_task_links_engine_code() {
    let files = control_sources();
    assert!(
        !files.is_empty(),
        "the scan found no sources under src/dags/control — the gate is checking nothing"
    );

    // Scanner self-check: the needle logic must actually match.
    assert!(
        code_lines("let s = er_cli::open_lake(a, b);").any(|(_, l)| l.contains(NEEDLES[0])),
        "the needle scan is broken"
    );

    let mut offenders = Vec::new();
    for path in &files {
        let src = fs::read_to_string(path).expect("a readable control source");
        // Positive control: these are the control-task files, not strays.
        assert!(
            src.contains("ControlTask"),
            "{} does not implement ControlTask — is the directory still the control module?",
            path.display()
        );
        for (line, text) in code_lines(&src) {
            for needle in NEEDLES {
                if text.contains(needle) {
                    offenders.push(format!("{}:{line}: {needle}", path.display()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "control-task sources name engine code; a body that needs a lake open is a pod task \
         (§3.5):\n  {}",
        offenders.join("\n  ")
    );
}
