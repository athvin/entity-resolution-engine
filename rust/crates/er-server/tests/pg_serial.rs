//! Every `pg_*.rs` integration test must be ONE `#[tokio::test]`.
//!
//! **The reason changed, the rule did not.** These files no longer share a
//! schema — `common::fresh_db` gives each one its own database. But it is
//! keyed on one slug per *file*, so two tests in a file would both ask for the
//! same database, and the second `DROP DATABASE ... WITH (FORCE)` would
//! disconnect the first mid-test. The race moved from the tables to the
//! database itself; it did not go away.
//!
//! The original failure is worth keeping on record, because it is what a race
//! here looks like from the outside. `pg_segments.rs` had two tests, and the
//! second asserted that a *refused* config version never became active. It
//! read the *first* test's deliberately-valid published config instead and
//! failed with `left: 200, right: 422`, pointing at the publish gate, which was
//! working perfectly. It surfaced only in the DuckLake tier — the one CI job
//! with a Postgres — while the `test` job self-skipped and stayed green, so the
//! file looked fine locally for as long as nobody set the DSN.
//!
//! The convention that avoids it is sequential phases in one test, which is
//! what every other file in this directory already does. If a file ever
//! genuinely needs two concurrent tests, the fix is a per-test slug rather than
//! a per-file one — change `fresh_db`'s contract, then this gate.
//!
//! Needs no database: it reads source.

use std::path::Path;

/// Counts lines that *begin* with the attribute, which a real one always does.
///
/// Not `contains`: this file names `#[tokio::test]` in a string literal twice —
/// in this very filter and in the failure message — and four test files
/// deliberately mention it in prose to explain the convention. A `contains`
/// scan reported this gate as the only offender, so it would have had to skip
/// itself to pass, and a source scanner excluded from its own scan is one file
/// nobody is checking.
fn attribute_lines(src: &str) -> usize {
    src.lines()
        .filter(|l| l.trim_start().starts_with("#[tokio::test"))
        .count()
}

#[test]
fn every_postgres_test_file_is_a_single_test() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut offenders = Vec::new();
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("tests/ is readable") {
        let path = entry.expect("a readable dir entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let is_rust = path.extension().is_some_and(|e| e == "rs");
        if !name.starts_with("pg_") || !is_rust {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("a readable test file");
        let n = attribute_lines(&src);
        if n > 0 {
            checked += 1;
        }
        if n > 1 {
            offenders.push(format!("{name} declares {n}"));
        }
    }
    assert!(
        checked > 10,
        "the scan found only {checked} Postgres tests, so it is probably looking in the \
         wrong place and would pass no matter what"
    );
    assert!(
        offenders.is_empty(),
        "these Postgres test files declare more than one #[tokio::test], and the tests in \
         one binary run concurrently while `common::fresh_db` keys the database on one slug \
         per FILE — so they would race on the database itself, the second one dropping it \
         under the first: {}\n\nMake the extra ones plain `async fn` phases called in \
         sequence from the single test, each with its own org name. See the module docs \
         here for the failure this produces.",
        offenders.join(", ")
    );
}
