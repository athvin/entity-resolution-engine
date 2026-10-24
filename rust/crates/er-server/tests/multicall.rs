//! The deployed image ships ONE binary under three names. These tests hold that
//! contract.
//!
//! `rust/Dockerfile` builds only `er-server` and symlinks `er` and
//! `er-dispatcher` to it, because all three used to be separate binaries that
//! each statically linked their own copy of DuckDB. Dispatch is on `argv[0]`
//! (see the module header of `src/main.rs`), which is invisible until it breaks:
//! a refactor that moved the dispatch after the subcommand match, or renamed the
//! `er` library entry point, would leave `er …` silently running the HTTP server
//! and the chart would keep working, so nothing else would notice.
//!
//! Symlinks are the mechanism under test, so these tests are Unix-only — which
//! is also the only platform the image targets.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// A directory holding the built `er-server` plus the two symlinks the image
/// creates, so each name can be invoked exactly as a pod would invoke it.
fn multicall_dir(tag: &str) -> PathBuf {
    let exe = Path::new(env!("CARGO_BIN_EXE_er-server"));
    let dir = std::env::temp_dir().join(format!("er-multicall-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    // A hard copy, not a symlink to the target dir: argv[0] has to be the new
    // name, and copying keeps the test independent of how cargo laid the binary
    // out.
    std::fs::copy(exe, dir.join("er-server")).expect("copy er-server");
    for name in ["er", "er-dispatcher"] {
        std::os::unix::fs::symlink("er-server", dir.join(name)).expect("symlink");
    }
    dir
}

#[test]
fn invoked_as_er_it_is_the_pipeline_cli() {
    let dir = multicall_dir("er");
    let out = Command::new(dir.join("er"))
        .arg("--help")
        .output()
        .expect("runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Entity resolution over a DuckLake lake"),
        "`er --help` did not print the pipeline CLI's help; it printed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The stage subcommands are the actual surface operators use.
    for sub in [
        "ingest",
        "standardize",
        "match",
        "reconcile",
        "golden",
        "doctor",
    ] {
        assert!(
            stdout.contains(sub),
            "`er --help` is missing the {sub} subcommand:\n{stdout}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn invoked_as_er_dispatcher_it_is_the_dispatcher_daemon() {
    let dir = multicall_dir("disp");
    // With no DSN the daemon refuses to start, and the message is specific to
    // the dispatcher path — which is how we know we reached it rather than the
    // HTTP server (whose missing-DSN behaviour is to serve anyway).
    let out = Command::new(dir.join("er-dispatcher"))
        .env_remove("ER_PG_DSN")
        .output()
        .expect("runs");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("ER_PG_DSN is required for the dispatcher"),
        "`er-dispatcher` did not reach the dispatcher entry point; it said:\n{combined}"
    );
    assert!(!out.status.success(), "a dispatcher with no DSN must fail");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn invoked_as_er_server_the_subcommands_still_work() {
    let dir = multicall_dir("srv");
    let out = Command::new(dir.join("er-server"))
        .arg("install-extensions")
        .output()
        .expect("runs");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("--dir is required"),
        "`er-server install-extensions` did not reach its argument parsing:\n{combined}"
    );
    assert!(!out.status.success());

    // An unknown subcommand must NOT be swallowed into "serve the API": that is
    // how a typo in a chart template would turn into a silently wrong pod.
    let out = Command::new(dir.join("er-server"))
        .args(["migrate-tenants", "--nonsense"])
        .output()
        .expect("runs");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("unknown argument"),
        "unexpected output for a bad flag:\n{combined}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
