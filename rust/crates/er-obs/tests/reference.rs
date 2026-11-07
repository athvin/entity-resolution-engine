//! The observability skill's `references/telemetry-reference.md` is generated
//! from the registry, byte for byte.
//!
//! ## Why a byte-compare and not a hand-written doc
//!
//! `er-entity-lint/src/lib.rs` opens with the confession this pattern exists
//! for: hand-typed rule references are *"how three documented codes ended up
//! never implemented."* A metrics table has that failure mode and a worse one —
//! a documented metric nobody emits reads as **coverage**, because its dashboard
//! panel renders an honest-looking empty chart rather than an error.
//!
//! So the registry is the source of truth and the doc is its output. Update it
//! the way `er-entity-lint/tests/fixtures.rs` does:
//!
//! ```text
//! UPDATE_EXPECT=1 cargo test -p er-obs --test main reference
//! ```
//!
//! ## Why this is NOT wired into `er-entity verify`
//!
//! Deliberate. `entities` is a required status check that branch protection
//! points at for *schema* drift, and a span or metric rename is not schema. A
//! telemetry rename failing the entity gate would train the next person to
//! regenerate entity artifacts to fix an unrelated thing, which is how a drift
//! gate loses its meaning. This runs in `test`, with its own name.

use std::fs;
use std::path::PathBuf;

fn doc_path() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../.claude/skills/observability/references/telemetry-reference.md"
    ))
}

#[test]
fn the_reference_doc_matches_the_registry() {
    let rendered = er_obs::registry::reference();
    let path = doc_path();

    if std::env::var_os("UPDATE_EXPECT").is_some() {
        fs::write(&path, &rendered).expect("writable telemetry-reference.md");
        return;
    }

    let committed = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}. Generate it with \
             `UPDATE_EXPECT=1 cargo test -p er-obs --test main reference`",
            path.display()
        )
    });

    assert_eq!(
        committed, rendered,
        "the committed telemetry-reference.md is stale. Regenerate it with \
         `UPDATE_EXPECT=1 cargo test -p er-obs --test main reference` and commit \
         the diff — the registry in `er-obs/src/registry.rs` is the source of \
         truth, and this file is its output."
    );
}

#[test]
fn the_rendered_doc_says_it_is_generated() {
    // A generated file that does not announce itself gets hand-edited once,
    // and the edit is then silently reverted by the next regeneration — which
    // is worse than either outcome on its own.
    let rendered = er_obs::registry::reference();
    assert!(rendered.contains("GENERATED"), "the header must say so");
    assert!(
        rendered.contains("UPDATE_EXPECT=1"),
        "the header must say how to regenerate it"
    );
}

#[test]
fn every_declared_metric_appears_in_the_doc() {
    // The property that makes the doc trustworthy: not merely that it is
    // current, but that it is complete. A renderer that silently skipped a
    // metric would still byte-compare clean against its own output.
    let rendered = er_obs::registry::reference();
    for metric in er_obs::registry::METRICS {
        assert!(
            rendered.contains(metric.name),
            "`{}` is declared but missing from the rendered doc",
            metric.name
        );
        assert!(
            rendered.contains(metric.description),
            "`{}`'s description is missing from the rendered doc",
            metric.name
        );
    }
    for dimension in er_obs::registry::FORBIDDEN_DIMENSIONS {
        assert!(
            rendered.contains(dimension),
            "forbidden dimension `{dimension}` is missing from the rendered doc"
        );
    }
}
