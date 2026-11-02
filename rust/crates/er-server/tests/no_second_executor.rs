//! The posterity gate for the one-executor rule (§11): the Kubernetes test
//! double lives in `tests/`, and this scan is what keeps it there. If a file
//! under `er-server/src` or `er-k8s/src` ever names `FakePodApi` or
//! `fake_pod_api`, a second executor is growing inside the product — the §6.9
//! boundary says the double may fake the CLUSTER, never the dispatcher.
//!
//! Beside `egress_guard.rs`, which makes the same kind of "this cannot come
//! back" claim structurally rather than by review.

use std::fs;
use std::path::{Path, PathBuf};

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
fn the_fake_pod_api_never_enters_the_product() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    sources(&crates.join("er-server/src"), &mut files);
    sources(&crates.join("er-k8s/src"), &mut files);
    assert!(files.len() > 10, "the scan found the source tree");

    // Comment lines may NAME the double (docs pointing a reader at it);
    // only code reaching it is a leak — the same filter egress_guard.rs uses.
    let mut hits = Vec::new();
    for path in files {
        let text = fs::read_to_string(&path).expect("readable source");
        for (i, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if line.contains("FakePodApi") || line.contains("fake_pod_api") {
                hits.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "the Kubernetes test double is tests-only; it leaked into the product:\n{}",
        hits.join("\n")
    );
}
