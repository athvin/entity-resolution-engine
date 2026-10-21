//! The posterity gate for the outbound-URL guard.
//!
//! `webhook::emit_with_egress` takes an [`Egress`] so that the delivery
//! integration test can POST to a listener on `127.0.0.1` — which is exactly
//! what the guard exists to block. That seam is the only way past the guard,
//! and it must stay a test-only seam.
//!
//! There is no environment variable and no config field for this on purpose: a
//! guard that configuration can switch off is a guard that gets found switched
//! off. The cost of that choice is a bypass-shaped function in the public API,
//! and this scan is what pays it — if a file under `er-server/src` ever names
//! the unguarded entry points or `Egress::Unguarded`, this fails and says where.
//!
//! Beside `freeform_sql.rs` and `declared_names.rs`, which make the same kind of
//! "this cannot come back" claim structurally rather than by review.

use std::fs;
use std::path::{Path, PathBuf};

/// `(line number, text)` for the lines of `text` that are not comments.
///
/// These scans look for names in source, and the names they look for are also
/// the names the explanations use — this file's own prose says
/// `reqwest::Client::new()` while telling you not to call it. Line-leading `//`
/// is all that is filtered: it is enough for a deliberate textual gate, and a
/// real parser here would be more machinery than the rule is worth.
fn code_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
}

/// Every `.rs` file under `dir`, skipping `target/` and dotted dirs.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("readable source dir") {
        let path = entry.expect("dir entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if name == "target" || name.starts_with('.') {
                continue;
            }
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_product_code_bypasses_the_outbound_url_guard() {
    let src = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
    let mut files = Vec::new();
    sources(&src, &mut files);
    // A broken walk that finds nothing must fail loudly, not pass silently.
    assert!(
        files.len() >= 5,
        "the er-server/src scan looks wrong ({} files under {})",
        files.len(),
        src.display()
    );

    const BYPASSES: [&str; 3] = [
        "Egress::Unguarded",
        "emit_with_egress",
        "emit_job_event_with_egress",
    ];
    let mut offenders: Vec<String> = Vec::new();
    for file in &files {
        // The guard module and the webhook module DEFINE the seam; they are the
        // two files allowed to name it.
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "urlguard.rs" || name == "webhook.rs" {
            continue;
        }
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        for (lineno, line) in code_lines(&text) {
            for needle in BYPASSES {
                if line.contains(needle) {
                    offenders.push(format!("{}:{lineno}: {needle}", file.display()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "product code must not reach the unguarded outbound path — call \
         webhook::emit / webhook::emit_job_event, which always apply the \
         guard:\n{}",
        offenders.join("\n")
    );
}

/// Redirects are the other way past the guard, and the default policy allows
/// them.
///
/// `reqwest::Client::new()` follows up to ten redirects, so a webhook at
/// `https://public.example/` that 302s to `http://169.254.169.254/...` defeats a
/// guard that only inspected the URL it was given. Every client in this crate is
/// a webhook-delivery client, so the rule is simply that nobody constructs one
/// with the default policy — `webhook::client()` is the only way to get one.
#[test]
fn no_product_code_builds_an_http_client_that_follows_redirects() {
    let src = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
    let mut files = Vec::new();
    sources(&src, &mut files);
    assert!(files.len() >= 5, "the er-server/src scan looks wrong");

    let mut offenders: Vec<String> = Vec::new();
    for file in &files {
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        for (lineno, line) in code_lines(&text) {
            if line.contains("reqwest::Client::new()") {
                offenders.push(format!("{}:{lineno}", file.display()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "use webhook::client(), which disables redirect-following — \
         reqwest's default follows up to ten hops and the URL guard only sees \
         the first:\n{}",
        offenders.join("\n")
    );

    let webhook = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/webhook.rs"))
        .expect("webhook.rs is readable");
    assert!(
        webhook.contains("redirect(reqwest::redirect::Policy::none())"),
        "webhook::client must disable redirect-following"
    );
}

/// The guarded entry points must actually BE guarded — i.e. `webhook.rs` must
/// route its public `emit` through the guard rather than past it. A scan for the
/// bypass names proves nothing if the guarded wrapper forgot to check.
#[test]
fn the_guarded_entry_points_call_the_guard() {
    let webhook = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/webhook.rs"))
        .expect("webhook.rs is readable");
    assert!(
        webhook.contains("urlguard::check(&url, egress)"),
        "the delivery loop must re-check every URL before POSTing to it"
    );
    assert!(
        webhook.contains("Egress::Guarded"),
        "the public emit wrappers must pass Egress::Guarded"
    );
    let lib = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
        .expect("lib.rs is readable");
    assert!(
        lib.contains("urlguard::check(&body.url"),
        "webhook registration must check the URL it is about to store"
    );
}
