//! Validates that the committed golden fixtures match MANIFEST.json exactly —
//! catching accidental edits or partial regeneration of the golden baselines.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

fn golden_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/golden")
}

fn sha256_hex(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    hex::encode(Sha256::digest(bytes))
}

#[test]
fn goldens_match_manifest() {
    for dataset in ["fake_1000", "historical_50k", "exploding"] {
        check_manifest(&golden_root().join(dataset));
    }
}

fn check_manifest(dir: &Path) {
    let manifest_path = dir.join("MANIFEST.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).expect("read MANIFEST.json"))
            .expect("parse MANIFEST.json");

    let files: BTreeMap<String, String> =
        serde_json::from_value(manifest["files"].clone()).expect("files map");
    assert!(!files.is_empty(), "manifest lists no files");

    let mut on_disk: Vec<String> = Vec::new();
    collect_files(dir, dir, &mut on_disk);

    for (rel, expected) in &files {
        let actual = sha256_hex(&dir.join(rel));
        assert_eq!(
            &actual, expected,
            "golden file drifted from manifest: {rel}"
        );
    }

    let extras: Vec<&String> = on_disk
        .iter()
        .filter(|rel| *rel != "MANIFEST.json" && !files.contains_key(*rel))
        .collect();
    assert!(
        extras.is_empty(),
        "files on disk missing from manifest: {extras:?}"
    );
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_files(root, &path, out);
        } else {
            out.push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
}
