//! The committed `entities/` tree holds every rule.
//!
//! This test is why moving the conventions out of `Catalog::load_dir` is safe.
//! `load_dir` used to be the gate, so any code path that loaded a catalog got
//! the V-rules for free; now `er-entity verify` is the gate, and this test is
//! what guarantees the tree it gates over actually conforms — including for the
//! library consumers (the generator's tests, the differ's tests) that still call
//! `load_dir` directly.
//!
//! It is the entity-level analogue of `golden_read_statements_pass_every_rule`
//! in er-sql-check.

use std::path::PathBuf;

use er_entity_lint::{Severity, lint_catalog};
use er_entity_spec::Catalog;

fn entities_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../entities"))
}

/// Every `org_id` table that has not yet earned a plain leading-`org_id` index
/// (V15, a warning). Listed exactly rather than counted so that adding a
/// tenant-scoped table without an index fails here, and so that fixing one
/// requires deleting a line — which is the only way this list ever shrinks.
const V15_KNOWN: &[&str] = &["api_key", "config_version", "job", "schedule", "webhook"];

#[test]
fn the_committed_tree_has_no_lint_errors() {
    let dir = entities_dir();
    let catalog = Catalog::load_dir(&dir).expect("the entities tree loads");
    let errors: Vec<_> = lint_catalog(&catalog, &dir)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "the committed entities/ tree must lint clean:\n{}",
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn the_committed_trees_warnings_are_exactly_the_known_debt() {
    let dir = entities_dir();
    let catalog = Catalog::load_dir(&dir).expect("the entities tree loads");
    let warned: Vec<String> = lint_catalog(&catalog, &dir)
        .into_iter()
        .filter(|d| d.severity == Severity::Warning && d.code.0 == "V15")
        .map(|d| d.entity)
        .collect();
    let mut expected: Vec<String> = V15_KNOWN.iter().map(ToString::to_string).collect();
    expected.sort();
    assert_eq!(
        warned, expected,
        "V15's findings moved. If a table gained an org_id index, delete it from \
         V15_KNOWN; if a new tenant-scoped table landed without one, give it an index."
    );
}

#[test]
fn every_entity_lives_in_a_directory_named_for_it() {
    // lint_catalog builds a diagnostic's path from the entity's `name`, so a
    // directory whose name differs would make findings point at a file that
    // does not exist. The loader does not enforce this (it discards the path),
    // which is exactly why it is worth asserting.
    let dir = entities_dir();
    let catalog = Catalog::load_dir(&dir).expect("the entities tree loads");
    for entity in catalog.entities() {
        let expected = dir.join(&entity.name).join("entity.yaml");
        assert!(
            expected.exists(),
            "entity {:?} does not live at {}",
            entity.name,
            expected.display()
        );
    }
}
