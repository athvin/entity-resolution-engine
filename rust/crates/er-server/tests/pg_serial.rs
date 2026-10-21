//! Every `pg_*.rs` integration test must be ONE `#[tokio::test]`.
//!
//! Cargo runs the tests inside one binary concurrently, and every Postgres test
//! in this crate shares a single schema which it drops and recreates on the way
//! in. Two tests in one file are therefore two tests racing on the same tables:
//! one truncates the world out from under the other, or — worse, because it
//! looks like a product bug — leaves state behind that the other asserts
//! against.
//!
//! That is not hypothetical. `pg_segments.rs` had two, and the second one
//! asserted that a *refused* config version never became active. It read the
//! *first* test's deliberately-valid published config instead and failed with
//! `left: 200, right: 422`, pointing at the publish gate, which was working
//! perfectly. It surfaced only in the DuckLake tier — the one CI job with a
//! Postgres — while the `test` job self-skipped and stayed green, so the file
//! looked fine locally for as long as nobody set the DSN.
//!
//! The convention that avoids it is sequential phases in one test, which is
//! what the other twenty-odd files in this directory already do. This gate
//! keeps the next one from drifting, because the failure it produces does not
//! look like a test-isolation problem.
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
         one binary run concurrently against one shared schema: {}\n\nMake the extra ones \
         plain `async fn` phases called in sequence from the single test, each with its own \
         org name. See the module docs here for the failure this produces.",
        offenders.join(", ")
    );
}
