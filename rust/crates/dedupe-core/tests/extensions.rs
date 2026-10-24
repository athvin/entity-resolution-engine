//! DuckDB extension loading via `SessionOptions`. The name-validation guard and
//! the baked-directory behaviour are checked offline; actually loading
//! `rapidfuzz` needs a network INSTALL, so that test is gated behind
//! `DEDUPE_E2E_EXTENSIONS=1`.

use dedupe_core::session::{BAKED_EXTENSIONS, Session, SessionOptions};

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
fn a_baked_extension_directory_suppresses_the_network_install() {
    // Point at an EMPTY directory. With a baked directory configured, the
    // session must not reach the extension repository — so attaching a lake,
    // which needs `ducklake`, has to fail rather than quietly download 27 MB.
    //
    // This is the offline half of the x2 arm in
    // scripts/measure_runner_startup.sh: that one proves the deployed image
    // needs no egress, this one proves the *mechanism* is what makes it so, and
    // fails if anyone re-adds an unconditional INSTALL to `attach_ducklake`.
    let dir = std::env::temp_dir().join(format!("er-empty-extdir-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut session = Session::open_in_memory(SessionOptions {
        extension_directory: Some(dir.to_string_lossy().into_owned()),
        ..SessionOptions::deterministic()
    })
    .expect("a session opens even with an empty extension directory");

    let result = session.attach_ducklake(&dedupe_core::DuckLakeConfig {
        metadata: dir.join("probe.ducklake").to_string_lossy().into_owned(),
        data_path: Some(dir.join("data").to_string_lossy().into_owned()),
        ..dedupe_core::DuckLakeConfig::default()
    });
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        result.is_err(),
        "with a baked extension directory set, LOAD must fail on a missing \
         extension instead of installing it over the network"
    );
}

#[test]
fn the_baked_list_is_installable_identifiers() {
    // The list is interpolated into `INSTALL <name>;` by
    // `er-server install-extensions`, so it may only ever hold bare
    // identifiers — the same rule `load_extensions` enforces for the
    // per-config extensions.
    // At least the four bisected entries, below.
    assert!(
        BAKED_EXTENSIONS.len() >= 4,
        "the baked list was emptied out"
    );
    for ext in BAKED_EXTENSIONS {
        assert!(
            !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "baked extension {ext:?} is not a bare identifier"
        );
    }
    // ducklake is the one the engine loads explicitly; the rest are autoloaded
    // out of the baked directory.
    assert!(BAKED_EXTENSIONS.contains(&"ducklake"));
    // Bisected in a full run-all with autoinstall off: dropping either of these
    // breaks the lake-schema reconcile.
    assert!(BAKED_EXTENSIONS.contains(&"parquet"));
    assert!(BAKED_EXTENSIONS.contains(&"json"));
    assert!(BAKED_EXTENSIONS.contains(&"icu"));
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
