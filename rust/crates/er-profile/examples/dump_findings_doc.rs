//! Regenerate the `er-engine-ops` skill's `references/profile-findings.md` from
//! the findings registry:
//!
//! ```sh
//! cargo run -q -p er-profile --example dump_findings_doc \
//!   > .claude/skills/er-engine-ops/references/profile-findings.md
//! ```
//!
//! The reference stays a committed, reviewed artifact — this tool exists so a
//! deliberate rule change regenerates it mechanically instead of by hand
//! surgery, and `tests/reference_doc.rs` byte-compares the result.

fn main() {
    print!("{}", er_profile::reference_markdown());
}
