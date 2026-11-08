//! The webhook signature contract says the same thing in all three places it
//! lives.
//!
//! ## The failure this prevents
//!
//! The signature header is named in `er-server`'s `SIGNATURE_HEADER` const, in
//! the tenant-facing webhooks UI, and in `connectors.md` §3.2 — three
//! languages, no shared definition, and for months the doc named
//! `X-ERServer-Signature` while the code sent `X-ER-Signature` (#763). A
//! receiver built from the doc read a header that was never sent. Nothing
//! compared the three, which is exactly why it drifted; this scanner is the
//! comparison.
//!
//! The gate shape follows `collector_allowlist.rs`: read the sources as text,
//! assert the agreement, fail with the fix instruction. It was observed
//! failing against the pre-fix doc before landing.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.."))
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The header name the code sends, from `pub const SIGNATURE_HEADER: &str = "…"`.
fn code_header() -> String {
    let source = read("rust/crates/er-server/src/webhook.rs");
    source
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix(r#"pub const SIGNATURE_HEADER: &str = ""#)?
                .strip_suffix("\";")
                .map(str::to_string)
        })
        .expect("webhook.rs declares SIGNATURE_HEADER — has it moved or been renamed?")
}

#[test]
fn doc_and_ui_name_the_header_the_code_sends() {
    let header = code_header();
    let doc = read(".claude/skills/er-server-control-plane/references/connectors.md");
    let ui = read("frontend/src/components/settings/webhooks-content.tsx");

    assert!(
        doc.contains(&header),
        "connectors.md §3.2 does not name the header the code sends ({header}). \
         A receiver built from the doc reads a header that is never sent. Fix the \
         doc — do not rename the const: the header is live in delivered webhooks \
         (#763)."
    );
    assert!(
        !doc.contains("X-ERServer-Signature"),
        "connectors.md still names X-ERServer-Signature, which the code has never \
         sent — the drift #763 fixed. The header is {header}."
    );
    assert!(
        ui.contains(&header),
        "webhooks-content.tsx does not name {header}; the tenant-facing UI and the \
         code disagree about the signature header."
    );
}

#[test]
fn doc_states_the_value_format_and_body_rule() {
    let doc = read(".claude/skills/er-server-control-plane/references/connectors.md");
    assert!(
        doc.contains("sha256=<hex>"),
        "connectors.md §3.2 does not state the signature value format \
         `sha256=<hex>` — a receiver that hex-compares the whole header value \
         fails on the prefix (webhook.rs `sign`)."
    );
    assert!(
        doc.contains("raw request body"),
        "connectors.md §3.2 does not say the signature is over the raw request \
         body — a receiver that re-serializes the JSON first computes a digest \
         over bytes that were never signed."
    );
}
