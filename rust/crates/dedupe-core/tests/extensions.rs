//! DuckDB extension loading via `SessionOptions`. The name-validation guard is
//! checked offline; actually loading `rapidfuzz` needs a network INSTALL, so
//! that test is gated behind `DEDUPE_E2E_EXTENSIONS=1`.

use dedupe_core::session::{Session, SessionOptions};

fn ext_enabled() -> bool {
    std::env::var("DEDUPE_E2E_EXTENSIONS").as_deref() == Ok("1")
}

#[test]
fn rejects_invalid_extension_name() {
    // Validation runs before any INSTALL/LOAD, so this needs no network and
    // blocks SQL injection via the extension name.
    let result = Session::open_in_memory(SessionOptions {
        extensions: vec!["rapidfuzz; drop table x".into()],
        ..SessionOptions::deterministic()
    });
    assert!(
        result.is_err(),
        "a non-identifier extension name must be rejected"
    );
}

#[test]
fn loads_rapidfuzz_and_exposes_its_functions() {
    if !ext_enabled() {
        eprintln!("skipping extension-load test (set DEDUPE_E2E_EXTENSIONS=1 to run)");
        return;
    }
    let session = Session::open_in_memory(SessionOptions {
        extensions: vec!["rapidfuzz".into()],
        install_extensions: true,
        ..SessionOptions::deterministic()
    })
    .unwrap();

    // token_set_ratio ignores token order, so reordered names score ~100 — a
    // function DuckDB lacks natively, now usable directly in comparison SQL.
    let score = session
        .query_scalar_f64("SELECT rapidfuzz_token_set_ratio('john smith', 'smith john')")
        .unwrap();
    assert!(
        score > 99.0,
        "token_set_ratio of reordered tokens should be ~100, got {score}"
    );
}
