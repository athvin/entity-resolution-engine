//! The telemetry layering rule: only a crate that owns an entry point may
//! depend on `er-obs`.
//!
//! ## What the split buys, and what breaks it
//!
//! `er-span` is the leaf — `tracing` and nothing else. It emits. `er-obs`
//! **collects**: the subscriber, the OTLP exporter, the metric instruments, the
//! `Layer`s, and with them `opentelemetry_sdk`, `opentelemetry-otlp`,
//! `tracing-opentelemetry`, `tonic` and a `reqwest`. That is the whole reason
//! the two crates are separate: instrumenting the engine must not put an
//! exporter and an HTTP stack into the compile of `dedupe-core`, the 42
//! generated modules of `er-entities`, or every test binary in the workspace.
//!
//! One line in one `Cargo.toml` undoes it. `cargo add er-obs` in a library
//! crate compiles fine, passes clippy, passes every test, and silently moves a
//! dozen dependencies into everything downstream of it.
//!
//! ## Why a gate rather than the module docs
//!
//! Because the module docs got it **wrong**, in the direction that reads as
//! permission. `er-obs/src/lib.rs` said "only binaries depend on it" and
//! `er-span/src/lib.rs` says the binary installs the subscriber — but `er-cli`
//! and `er-server` each have a `lib.rs` *and* take `er-obs`, so "binary crate"
//! was never the line. The real rule is **owns an entry point**, and stating it
//! in prose is how it came to be stated incorrectly (fixed in #772 and #784).
//!
//! So the rule is read off the manifests, which cannot describe themselves
//! wrongly, and the observability skill derives the same split for its roles
//! table rather than asserting it.
//!
//! ## What this deliberately does not check
//!
//! That a library emits *through* `er-span` at all — absence of instrumentation
//! is not a layering violation, and a crate with nothing worth a span should
//! not be forced to declare a dependency. `er-schema/tests/structured_logging.rs`
//! covers the related but distinct question of whether `er-server` logs through
//! `tracing` rather than `eprintln!`.

use std::fs;
use std::path::{Path, PathBuf};

/// The `rust/crates` directory.
fn crates_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
}

/// Whether `crate_dir` ships something you can run.
///
/// Three spellings, because cargo accepts three: the conventional
/// `src/main.rs`, any `src/bin/*.rs`, and an explicit `[[bin]]` section. Only
/// checking the first would flag `er-server`'s extra binaries' crate as a
/// violation the moment someone moved a `main`.
fn owns_an_entry_point(crate_dir: &Path, manifest: &str) -> bool {
    crate_dir.join("src/main.rs").is_file()
        || crate_dir.join("src/bin").is_dir()
        || manifest.contains("[[bin]]")
}

/// Does this manifest declare `er-obs` as a dependency of the crate itself?
///
/// Matched at the start of a line so a mention inside a comment — this
/// workspace's manifests carry long explanatory ones — is not a declaration.
/// `dev-dependencies` are **in scope on purpose**: a test-only `er-obs` still
/// pulls the exporter into that crate's test compile, which is precisely the
/// cost being avoided. A crate needing the span-capture harness takes
/// `er-obs` with `features = ["test-support"]` and must own a binary like any
/// other, or the harness belongs behind `er-span` instead.
fn declares_er_obs(manifest: &str) -> bool {
    manifest
        .lines()
        .any(|l| l.starts_with("er-obs") || l.starts_with("\"er-obs\""))
}

#[test]
fn only_a_crate_that_owns_an_entry_point_depends_on_er_obs() {
    let crates = crates_dir();
    let mut manifests = Vec::new();
    for entry in fs::read_dir(&crates).expect("readable crates dir") {
        let dir = entry.expect("dir entry").path();
        let manifest = dir.join("Cargo.toml");
        if manifest.is_file() {
            manifests.push((dir, manifest));
        }
    }

    // A zero or a handful here would mean the scan examined almost nothing — a
    // crate-tree rename would otherwise report success having looked at
    // nothing, which is the failure mode this repo's other structural gates
    // each guard with a floor of their own.
    assert!(
        manifests.len() > 20,
        "found only {} crate manifests under {} — has the layout moved?",
        manifests.len(),
        crates.display()
    );

    let mut collectors = Vec::new();
    let mut offenders = Vec::new();
    for (dir, manifest_path) in &manifests {
        let manifest = fs::read_to_string(manifest_path).expect("readable manifest");
        if !declares_er_obs(&manifest) {
            continue;
        }
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<unnamed>")
            .to_string();
        collectors.push(name.clone());
        if !owns_an_entry_point(dir, &manifest) {
            offenders.push(name);
        }
    }

    // The gate must not be vacuous. If nothing depends on `er-obs` at all then
    // either the dependency was renamed or the detection broke, and an empty
    // offender list would read as compliance.
    assert!(
        !collectors.is_empty(),
        "no crate declares `er-obs`, which cannot be right — either the \
         dependency was renamed or `declares_er_obs` no longer matches the \
         manifest spelling, and this gate is passing while blind"
    );

    assert!(
        offenders.is_empty(),
        "these crates depend on `er-obs` without owning an entry point:\n\n  {}\n\n\
         `er-obs` is the COLLECTING half of the telemetry layer — it brings the \
         subscriber, the OTLP exporter, the metric instruments, and with them \
         `opentelemetry_sdk`, `opentelemetry-otlp`, `tracing-opentelemetry`, \
         `tonic` and a `reqwest`. A library taking it moves all of that into \
         everything downstream, including `er-entities`' generated modules and \
         every test binary in the workspace.\n\n\
         A library emits through `er-span`, which carries only `tracing`: use \
         `er_span::generator_span` / `sql_span` and let the binary install the \
         subscriber with `er_obs::init`. See the `observability` skill.",
        offenders.join("\n  ")
    );
}

#[test]
fn er_span_stays_the_leaf() {
    // The other half of the split, and the cheaper one to regress: `er-span`
    // may depend on `tracing` and nothing else. If it ever grows a second
    // dependency, every crate that emits a span pays for it — which is the
    // cost the two-crate arrangement exists to avoid, approached from the
    // other side.
    let manifest = fs::read_to_string(crates_dir().join("er-span/Cargo.toml"))
        .expect("er-span/Cargo.toml is readable");

    let deps: Vec<String> = manifest
        .lines()
        .skip_while(|l| l.trim() != "[dependencies]")
        .skip(1)
        .take_while(|l| !l.trim_start().starts_with('['))
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect();

    assert!(
        !deps.is_empty(),
        "parsed no dependencies from er-span/Cargo.toml — the section header \
         must have changed and this gate is no longer reading it"
    );

    let extras: Vec<&String> = deps.iter().filter(|l| !l.starts_with("tracing")).collect();
    assert!(
        extras.is_empty(),
        "`er-span` must stay the leaf of the telemetry layer, carrying only \
         `tracing`, but it now declares:\n\n  {}\n\n\
         Everything that emits a span depends on this crate — `dedupe-core` and \
         all 42 generated modules of `er-entities` — so a dependency added here \
         is added to the compile of every one of them. Collecting-side \
         machinery belongs in `er-obs`, which only entry points may take.",
        extras
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}
