//! Byte-exact parity of `config_hash` and the canonical document against a golden
//! dump from the real Python `er.config` over the committed `configs/*.yaml`
//! fixtures (vendored under `tests/fixtures/`). Regenerate with
//! `uv run python rust/crates/er-config/tests/gen_golden.py`.

use er_config::{canonical_document, config_hash, load_config_str};

fn fixture(name: &str) -> &'static str {
    match name {
        "default.yaml" => include_str!("fixtures/default.yaml"),
        "test.yaml" => include_str!("fixtures/test.yaml"),
        other => panic!("unknown fixture {other}"),
    }
}

#[test]
fn matches_python_golden() {
    let raw = include_str!("config_golden.json");
    let golden: serde_json::Value = serde_json::from_str(raw).unwrap();
    let cases = golden["cases"].as_array().unwrap();
    assert!(cases.len() >= 2, "expected the default + test cases");

    for case in cases {
        let name = case["name"].as_str().unwrap();
        let yaml = fixture(case["yaml"].as_str().unwrap());
        let config = load_config_str(yaml).unwrap_or_else(|e| panic!("{name}: {e}"));

        assert_eq!(
            canonical_document(&config),
            case["canonical_document"].as_str().unwrap(),
            "{name}: canonical document is not byte-identical to Python",
        );
        assert_eq!(
            config_hash(&config),
            case["config_hash"].as_str().unwrap(),
            "{name}: config_hash differs from Python",
        );
    }
}

/// A new optional block must be invisible to `config_hash` until a tenant
/// actually uses it, or every existing config silently re-hashes and every
/// tenant's next run is promoted to a full rebuild by the drift guard.
///
/// `segments` is the block under test here; the property is the one every
/// `skip_serializing_if` block in `Config` relies on, so this also guards the
/// next one added.
#[test]
fn an_unused_segments_block_does_not_move_the_config_hash() {
    let plain = load_config_str(fixture("test.yaml")).unwrap();
    let with_empty = load_config_str(&format!("{}\nsegments: {{}}\n", fixture("test.yaml")))
        .expect("an explicitly empty segments block parses");

    assert!(plain.segments.is_empty());
    assert!(with_empty.segments.is_empty());
    assert_eq!(
        config_hash(&plain),
        config_hash(&with_empty),
        "an empty segments block must hash like no segments block at all"
    );
    assert!(
        !canonical_document(&plain).contains("segments"),
        "an unused block must not appear in the canonical document"
    );
}

/// And a *used* one must change the hash, or republishing a config with a new
/// segment would not be recognised as a change at all.
#[test]
fn a_declared_segment_parses_and_does_move_the_config_hash() {
    let plain = load_config_str(fixture("test.yaml")).unwrap();
    let scoped = load_config_str(&format!(
        "{}\nsegments:\n  west: \"addr_region in ('CA','OR','WA')\"\n  by_source: \"source_system = 'crm'\"\n",
        fixture("test.yaml")
    ))
    .expect("a segments block parses");

    assert_eq!(scoped.segments.len(), 2);
    assert_eq!(
        scoped.segments.get("west").map(String::as_str),
        Some("addr_region in ('CA','OR','WA')")
    );
    assert_ne!(
        config_hash(&plain),
        config_hash(&scoped),
        "declaring a segment is a config change and must hash differently"
    );
    assert!(canonical_document(&scoped).contains("segments"));
}
