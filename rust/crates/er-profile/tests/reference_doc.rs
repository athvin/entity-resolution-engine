//! The generated-reference gate: the `er-engine-ops` skill's
//! `references/profile-findings.md` must be exactly
//! what the registry renders.
//!
//! The point of generating it is that a rule reference which can disagree with
//! the rules is worse than none at all — a reader is entitled to rely on it. So
//! the file is an artifact, this test is the gate, and
//! `examples/dump_findings_doc.rs` is the only way to move it.

use std::path::PathBuf;

fn doc_path() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../.claude/skills/er-engine-ops/references/profile-findings.md"
    ))
}

#[test]
fn the_committed_reference_matches_the_registry() {
    let path = doc_path();
    let committed =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let rendered = er_profile::reference_markdown();
    assert_eq!(
        committed, rendered,
        "the committed profile-findings.md is stale — regenerate it:\n  \
         cargo run -q -p er-profile --example dump_findings_doc \
         > .claude/skills/er-engine-ops/references/profile-findings.md"
    );
}

#[test]
fn every_registered_rule_appears_in_the_reference() {
    // Belt and braces against the byte-compare: if the renderer ever stopped
    // emitting a section, the byte-compare would still pass as long as the
    // committed file was regenerated from the same broken renderer.
    let rendered = er_profile::reference_markdown();
    for info in er_profile::rule_infos() {
        assert!(
            rendered.contains(&format!("## {} — ", info.code)),
            "{} has no section in the rendered reference",
            info.code
        );
        assert!(
            rendered.contains(info.explain),
            "{}'s reasoning is missing from the rendered reference",
            info.code
        );
    }
}
