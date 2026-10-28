//! DuckDB extension loading via `SessionOptions`. The name-validation guard,
//! the baked-directory behaviour and the engine's reported version are checked
//! offline; actually loading a community extension needs a network INSTALL, so
//! that test is gated behind `DEDUPE_E2E_EXTENSIONS=1`.
//!
//! The version guard belongs here because the version *is* the extension
//! mechanism: DuckDB keys every extension binary, and the directory it caches
//! them in, by the engine version it reports. See `rust/.cargo/config.toml`.

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

/// The engine version the bundled build reports, decoded from the pinned
/// `duckdb` crate version in `rust/Cargo.lock`.
///
/// The crate encodes the engine in its middle component as
/// `major * 10000 + minor * 100 + patch`, so `1.20000.0` is DuckDB `v2.0.0` and
/// `1.10506.1` was `v1.5.6`. Returns `None` for the legacy `1.4.x` scheme,
/// which predates the encoding and is not reachable from this pin.
fn engine_version_from_lockfile() -> Option<String> {
    // CARGO_MANIFEST_DIR is rust/crates/dedupe-core; the workspace root is rust/.
    let lock = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    let text = std::fs::read_to_string(&lock)
        .unwrap_or_else(|e| panic!("reading {}: {e}", lock.display()));

    let encoded = text
        .split("[[package]]")
        .find_map(|block| {
            let mut lines = block.lines().map(str::trim).filter(|l| !l.is_empty());
            if lines.next()? != "name = \"duckdb\"" {
                return None;
            }
            lines
                .next()?
                .strip_prefix("version = \"")?
                .split('.')
                .nth(1)?
                .parse::<u32>()
                .ok()
        })
        .expect("rust/Cargo.lock pins the duckdb crate");

    if encoded < 10_000 {
        return None;
    }
    Some(format!(
        "v{}.{}.{}",
        encoded / 10_000,
        (encoded % 10_000) / 100,
        encoded % 100
    ))
}

#[test]
fn the_engine_reports_a_released_version() {
    // Why this is a test and not a comment: the release label is hand-supplied
    // by CXXFLAGS in rust/.cargo/config.toml, because libduckdb-sys's `bundled`
    // build never passes one and the vendored source therefore reports the
    // in-tree development string. Both ways that can go wrong are silent at
    // compile time and expensive at run time, so both are asserted here.
    //
    // Needs no network: version() is a core function.
    let conn = duckdb::Connection::open_in_memory().expect("an in-memory DuckDB opens");
    let version: String = conn
        .query_row("SELECT version()", [], |r| r.get(0))
        .expect("version() is a core function");

    // (1) A `-dev` label is routed through `source_id` rather than the version
    // tag when DuckDB builds an extension URL, and nothing is published under a
    // source hash — so every BAKED_EXTENSIONS install 404s.
    assert!(
        !version.contains("-dev"),
        "the linked DuckDB reports {version:?}, a development label. DuckDB \
         resolves extension URLs through source_id instead of the version tag \
         for any -dev version, and no extension binary is published under a \
         source hash, so installing {} would 404. The release label comes from \
         CXXFLAGS in rust/.cargo/config.toml — check it still reaches the \
         bundled C++ build (cc prefers CXXFLAGS_<target>, and cargo's [env] \
         will not override a variable the environment already set).",
        BAKED_EXTENSIONS.join(", "),
    );

    // (2) A label that disagrees with the pin is worse than a -dev one: the
    // fetch succeeds and loads extensions built for a *different* engine. This
    // is what a `duckdb` bump without a matching config.toml edit produces.
    if let Some(expected) = engine_version_from_lockfile() {
        assert_eq!(
            version, expected,
            "the linked DuckDB reports {version:?} but the pinned duckdb crate \
             bundles {expected}. Extension binaries are published per engine \
             version, so this mismatch silently loads extensions built against \
             another engine. Update CXXFLAGS in rust/.cargo/config.toml to \
             match the pin in rust/Cargo.toml."
        );
    }
}

#[test]
fn loads_a_community_extension_and_exposes_its_functions() {
    if !ext_enabled() {
        eprintln!("skipping extension-load test (set DEDUPE_E2E_EXTENSIONS=1 to run)");
        return;
    }
    // `splink_udfs` rather than `rapidfuzz`, which this test used until DuckDB
    // 2.0. The community-repository build of rapidfuzz is now a stub that
    // throws at init: "The rapidfuzz extension for DuckDB 2.0 and later is
    // distributed from the Query.Farm extension repository, not the DuckDB
    // community repository." Reaching it would mean registering a third-party
    // extension repository, which is a supply-chain decision of its own and
    // buys nothing measurable — PERFORMANCE.md records rapidfuzz as the *same
    // speed* as DuckDB's built-ins for the functions this repo would use. Note
    // that the binary is still served with HTTP 200, so availability says
    // nothing here; only loading it does.
    //
    // splink_udfs is the better subject anyway: it still installs from the
    // community repository, it is deliberate Splink provenance rather than an
    // optional nicety, and `soundex`/`double_metaphone` are genuinely absent
    // from DuckDB, which is the property this test is really about.
    let session = Session::open_in_memory(SessionOptions {
        extensions: vec!["splink_udfs".into()],
        install_extensions: true,
        ..SessionOptions::deterministic()
    })
    .unwrap();

    // soundex collapses spelling variants to one code — a function DuckDB
    // lacks natively, now usable directly in comparison SQL.
    let codes = session
        .query_strings("SELECT soundex('Robert'), soundex('Rupert')")
        .unwrap();
    let row = codes.first().expect("one row");
    assert_eq!(
        row[0], row[1],
        "soundex should collapse Robert/Rupert to one code, got {row:?}"
    );
}
