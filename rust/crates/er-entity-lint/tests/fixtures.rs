//! The per-rule fixture harness.
//!
//! One directory per case under `tests/fixtures/`:
//!
//! ```text
//! tests/fixtures/V4_missing_updated_at/
//!   thing.yaml      one or more entities, each a whole `entity.yaml`
//!   expected.txt    the diagnostics they must produce, `[CODE] entity: message`
//! ```
//!
//! Every `*.yaml` in the directory is loaded into one catalog, so a whole-tree
//! rule (V9, V14) gets its fixture the same way a single-entity rule does. An
//! empty `expected.txt` asserts the case lints clean.
//!
//! Adding a rule means adding a directory here; nothing in this file changes.
//! Run with `UPDATE_EXPECT=1` to write `expected.txt` from the current
//! behaviour — then read the diff, because that file is the assertion.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use er_entity_lint::lint_catalog;
use er_entity_spec::{Catalog, Entity};

fn fixtures_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))
}

/// The fixture's diagnostics, one per line. Deliberately not `Diagnostic`'s own
/// `Display`: that renders an absolute path, which would make the expectation
/// depend on where the repo is checked out.
fn render(case: &Path) -> String {
    let mut entities: Vec<Entity> = Vec::new();
    let mut files: Vec<PathBuf> = fs::read_dir(case)
        .expect("fixture directory is readable")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "yaml"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "{} holds no *.yaml entity",
        case.display()
    );
    for file in &files {
        let text = fs::read_to_string(file).expect("fixture entity is readable");
        let entity: Entity = serde_norway::from_str(&text)
            .unwrap_or_else(|e| panic!("{} does not parse: {e}", file.display()));
        entities.push(entity);
    }
    let catalog = Catalog::from_entities(entities).unwrap_or_else(|e| {
        panic!(
            "{} is not structurally loadable, so no rule can be reached: {e}",
            case.display()
        )
    });
    let mut out = String::new();
    for diagnostic in lint_catalog(&catalog, case) {
        let _ = writeln!(
            out,
            "[{}] {}: {}",
            diagnostic.code, diagnostic.entity, diagnostic.message
        );
    }
    out
}

#[test]
fn every_fixture_matches_its_expectation() {
    let update = std::env::var_os("UPDATE_EXPECT").is_some();
    let mut cases: Vec<PathBuf> = fs::read_dir(fixtures_root())
        .expect("tests/fixtures exists")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    cases.sort();
    assert!(!cases.is_empty(), "no fixtures found");

    let mut failures = Vec::new();
    for case in &cases {
        let actual = render(case);
        let expected_path = case.join("expected.txt");
        if update {
            fs::write(&expected_path, &actual).expect("expected.txt is writable");
            continue;
        }
        let expected = fs::read_to_string(&expected_path).unwrap_or_else(|_| {
            panic!(
                "{} has no expected.txt (write one, or re-run with UPDATE_EXPECT=1)",
                case.display()
            )
        });
        if actual != expected {
            failures.push(format!(
                "--- {} ---\nexpected:\n{}\nactual:\n{}",
                case.file_name().unwrap_or_default().to_string_lossy(),
                expected,
                actual
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} fixture(s) disagree with their expectation:\n\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn every_rule_has_at_least_one_fixture() {
    // The point of the harness: a rule with no case is a rule nothing proves.
    let cases: Vec<String> = fs::read_dir(fixtures_root())
        .expect("tests/fixtures exists")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    for info in er_entity_lint::rules() {
        let prefix = format!("{}_", info.code);
        assert!(
            cases.iter().any(|case| case.starts_with(&prefix)),
            "{} has no fixture — add tests/fixtures/{}<case>/",
            info.code,
            prefix
        );
    }
}
