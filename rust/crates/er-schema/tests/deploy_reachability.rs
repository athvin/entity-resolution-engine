//! `deploy` is reachable ONLY through `images` (#661): deploy.yml's `push:`
//! trigger was a misdiagnosis (#587) and is gone, which makes `images.yml`'s
//! push paths the single road a platform change has to the cluster. This gate
//! holds both halves:
//!
//! - deploy.yml must not grow a `push:` trigger back — the misdiagnosis is
//!   written into its own header, and a re-add would resume putting noise-red
//!   `deploy` runs on `main` for every mixed commit.
//! - images.yml's push paths must keep covering every platform surface the
//!   deploy applies. Trimming one would make that surface's changes merge
//!   green and never deploy — the original #507 failure, through a third door.
//!
//! In this crate for the same reason `collector_allowlist.rs` is: a Rust test
//! reading repo YAML lands in the required `test` context, where the ungated
//! `workflow pins (S9)` job would let a regression merge.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("the repo root resolves")
}

fn workflow(name: &str) -> String {
    let path = repo_root().join(".github/workflows").join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// The triggers block of a workflow: everything between the top-level `on:`
/// line and the next top-level key. Textual, like the allowlist gate — the
/// question "does a key appear in this block" does not need a YAML parser, and
/// a cheap check that cannot silently stop working beats a clever one.
fn triggers_block(yaml: &str) -> &str {
    let start = yaml.find("\non:\n").expect("the workflow has an on: block");
    let rest = &yaml[start + 1..];
    let end = rest[4..]
        .find("\nconcurrency:")
        .or_else(|| rest[4..].find("\njobs:"))
        .or_else(|| rest[4..].find("\nenv:"))
        .or_else(|| rest[4..].find("\npermissions:"))
        .map_or(rest.len(), |i| i + 4);
    &rest[..end]
}

#[test]
fn deploy_has_no_push_trigger() {
    let yaml = workflow("deploy.yml");
    let triggers = triggers_block(&yaml);
    assert!(
        !triggers
            .lines()
            .any(|l| l.trim_start().starts_with("push:")),
        "deploy.yml grew a push: trigger back. #587 added one on a misdiagnosis and #661 \
         removed it — the real cause of the never-deployed allowlist was cancellation \
         (fixed by queueing), and the push path only ever produced noise-red deploy runs \
         racing their own image builds. The triggers block now reads:\n{triggers}"
    );
    // The two legitimate doors stay.
    assert!(triggers.contains("workflow_run:"), "{triggers}");
    assert!(triggers.contains("workflow_dispatch:"), "{triggers}");
}

#[test]
fn images_push_paths_cover_every_platform_surface() {
    let yaml = workflow("images.yml");
    let triggers = triggers_block(&yaml);
    // With deploy reachable only through images, each of these is a surface
    // whose edit MUST start an images run (which skips its builds and fires
    // the deploy). The list is the deploy's own inputs: the charts and
    // namespace provisioning, the observability installer, the deploy's
    // extracted scripts, and the workflow file carrying the helm flags.
    for required in [
        "infra/k8s/**",
        "scripts/dev_env.sh",
        "scripts/obs_up.sh",
        "scripts/ci/deploy-*.sh",
        ".github/workflows/deploy.yml",
    ] {
        assert!(
            triggers.contains(required),
            "images.yml's push paths no longer cover {required:?} — with deploy.yml's push \
             trigger gone (#661), this list is the ONLY road a change to that surface has \
             to the cluster. Removing it makes such changes merge green and never deploy, \
             which is the #507 failure returning. The triggers block now reads:\n{triggers}"
        );
    }
}
