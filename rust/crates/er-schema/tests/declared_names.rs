//! The posterity gate, static half: any qualified `lake.main.<relation>` literal
//! anywhere in the workspace must name a relation the S5 registry declares. A new
//! persistent lake relation therefore cannot ship by writing DDL inline — it has
//! to enter `er_schema::model` (or the entity catalog, whose lake entities are
//! oracled against the registry) first, or this test names the offending file.
//!
//! The live half is `er-cli`'s `lake_e2e::the_lake_contains_only_declared_relations`,
//! which catches dynamically-built names this scan cannot see.
//!
//! `dedupe-cli` is exempt: the standalone engine's CLI writes operator-named
//! outputs (`--output lake.main.<whatever>`) by design — that surface is outside
//! the ER platform's schema authorities, and the ER pipeline never uses it.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Every `.rs`/`.sql` file under `dir`, skipping `target/` and dotted dirs.
fn source_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("readable workspace dir") {
        let path = entry.expect("dir entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if name == "target" || name.starts_with('.') {
                continue;
            }
            source_files(&path, out);
        } else if path
            .extension()
            .is_some_and(|ext| ext == "rs" || ext == "sql")
        {
            out.push(path);
        }
    }
}

/// The relation names in `text`'s qualified `lake.main.<name>` occurrences.
fn lake_names(text: &str) -> BTreeSet<String> {
    const NEEDLE: &str = "lake.main.";
    let mut names = BTreeSet::new();
    let mut rest = text;
    while let Some(i) = rest.find(NEEDLE) {
        rest = &rest[i + NEEDLE.len()..];
        let end = rest
            .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(rest.len());
        if end > 0 {
            names.insert(rest[..end].to_string());
        }
    }
    names
}

#[test]
fn every_lake_main_literal_names_a_declared_relation() {
    let declared: BTreeSet<String> = er_schema::model::all_specs()
        .into_iter()
        .map(|s| s.name)
        .collect();

    let crates = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
    let mut files = Vec::new();
    source_files(&crates, &mut files);
    // A broken walk that finds almost nothing must fail loudly, not pass silently.
    assert!(
        files.len() > 100,
        "the workspace scan looks wrong ({} files found under {})",
        files.len(),
        crates.display()
    );

    let mut undeclared: Vec<String> = Vec::new();
    for file in &files {
        if file.components().any(|c| c.as_os_str() == "dedupe-cli") {
            continue; // operator-named outputs: the standalone engine's surface
        }
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        for name in lake_names(&text) {
            if !declared.contains(&name) {
                undeclared.push(format!("{}: lake.main.{name}", file.display()));
            }
        }
    }
    assert!(
        undeclared.is_empty(),
        "qualified lake relation names that the S5 registry does not declare — \
         add the relation to er_schema::model (or the entity catalog) before \
         referencing it:\n{}",
        undeclared.join("\n")
    );
}
