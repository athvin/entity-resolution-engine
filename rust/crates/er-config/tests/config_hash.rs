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
