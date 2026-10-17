//! Report breaking changes between a base OpenAPI document and the current one.
//!
//!   cargo run -p er-server --bin er-openapi-check -- <base.json> [--strict]
//!
//! Compares `<base.json>` (e.g. `git show origin/main:frontend/src/lib/api/openapi.json`)
//! against the document this build emits. It is **advisory** by default (always
//! exits 0, printing any breaking changes) — the gate blocks (exit 2) only with
//! `--strict`, which the release pipeline turns on once the first external org key
//! exists.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let strict = args.iter().any(|a| a == "--strict");
    let Some(base_path) = args.iter().skip(1).find(|a| !a.starts_with("--")) else {
        eprintln!("usage: er-openapi-check <base-openapi.json> [--strict]");
        return ExitCode::from(64);
    };
    let base = match std::fs::read_to_string(base_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("er-openapi-check: cannot read {base_path}: {e}");
            return ExitCode::from(64);
        }
    };
    let current = er_server::openapi::openapi_json();
    let breaks = er_server::openapi::breaking_changes(&base, &current);
    if breaks.is_empty() {
        println!("er-openapi-check: no breaking API changes");
        return ExitCode::SUCCESS;
    }
    eprintln!(
        "er-openapi-check: {} breaking API change(s) vs the base document:",
        breaks.len()
    );
    for b in &breaks {
        eprintln!("  - {b}");
    }
    if strict {
        ExitCode::from(2)
    } else {
        eprintln!("(advisory: not failing the build; pass --strict to enforce)");
        ExitCode::SUCCESS
    }
}
