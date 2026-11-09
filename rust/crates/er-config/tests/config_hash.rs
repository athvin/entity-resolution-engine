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

/// An unused `approval` block must hash like no block at all (O5c).
///
/// The same obligation `segments` carries, and for the same reason: every
/// existing tenant's `config_hash` has to be byte-identical after this field
/// exists, or adding an approval workflow to the TYPE would look like a config
/// change to every tenant that never asked for one.
#[test]
fn an_unused_approval_block_does_not_move_the_config_hash() {
    let plain = load_config_str(fixture("test.yaml")).unwrap();
    let with_empty = load_config_str(&format!("{}\napproval: {{}}\n", fixture("test.yaml")))
        .expect("an explicitly empty approval block parses");

    assert!(plain.approval.is_empty());
    assert!(with_empty.approval.is_empty());
    assert_eq!(
        config_hash(&plain),
        config_hash(&with_empty),
        "an empty approval block must hash like no approval block at all"
    );
    assert!(
        !canonical_document(&plain).contains("approval"),
        "an unused block must not appear in the canonical document"
    );
}

/// And a configured gate must move it, or turning the workflow on would not be
/// recognised as a change — the republish would reuse the old model and the old
/// behaviour.
#[test]
fn a_configured_approval_tag_moves_the_config_hash() {
    let plain = load_config_str(fixture("test.yaml")).unwrap();
    let gated = load_config_str(&format!(
        "{}\napproval:\n  tag: approved-for-merge\n",
        fixture("test.yaml")
    ))
    .expect("a configured approval block parses");

    assert_eq!(gated.approval.tag, "approved-for-merge");
    assert!(!gated.approval.is_empty());
    assert_ne!(config_hash(&plain), config_hash(&gated));
    assert!(canonical_document(&gated).contains("approved-for-merge"));
}

/// Whitespace is not a configuration. A tag of `"   "` would resolve against no
/// tag in the vocabulary and silently do nothing every tick.
#[test]
fn a_blank_approval_tag_is_no_approval_workflow() {
    let blank = load_config_str(&format!(
        "{}\napproval:\n  tag: \"   \"\n",
        fixture("test.yaml")
    ))
    .expect("a blank approval tag parses");
    assert!(blank.approval.is_empty());
}

#[test]
fn nonfinite_yaml_numbers_cannot_be_hashed_or_published() {
    let original = fixture("default.yaml");
    let finite = load_config_str(original).unwrap();
    let finite_hash = config_hash(&finite);

    for (number, path) in [
        (".nan", "thresholds.auto_merge"),
        (".inf", "thresholds.auto_merge"),
        ("-.inf", "thresholds.auto_merge"),
    ] {
        let yaml = original.replace("auto_merge: 0.95", &format!("auto_merge: {number}"));
        let error = load_config_str(&yaml).unwrap_err();
        assert!(error.to_string().contains(path), "{number}: {error}");
    }

    // Previously all three nonfinite values serialised as JSON null, so they
    // produced the same hash even though the config values were distinct.
    assert_eq!(
        config_hash(&load_config_str(original).unwrap()),
        finite_hash
    );

    for (yaml, path) in [
        (
            original.replace("review_low: 0.60", "review_low: .nan"),
            "thresholds.review_low",
        ),
        (
            original.replace(
                "review_low: 0.60",
                "review_low: 0.60\n  full_agreement_bits: .inf",
            ),
            "thresholds.full_agreement_bits",
        ),
        (
            original.replace(
                "review_low: 0.60",
                "review_low: 0.60\n  disagreement_ratio: -.inf",
            ),
            "thresholds.disagreement_ratio",
        ),
        (
            format!("{original}\nmodels:\n  - {{name: main, threshold: .nan}}\n"),
            "models[0].threshold",
        ),
        (
            original.replace(
                "addr_postal: { levels: [exact, null] }",
                "addr_postal: { levels: [exact, null], weight: .inf }",
            ),
            "comparisons.addr_postal.weight",
        ),
        (
            format!(
                "{original}\ncomparison_groups:\n  phones:\n    attributes: [phone, phone_e164]\n    levels: [exact]\n    weight: .nan\n"
            ),
            "comparison_groups.phones.weight",
        ),
        (
            original.replace("recall: 0.85", "recall: -.inf"),
            "training.recall",
        ),
        (
            original.replace(
                "cut_protect_probability: 1.0",
                "cut_protect_probability: .inf",
            ),
            "clustering.cut_protect_probability",
        ),
        (
            format!("{original}\ndata_quality:\n  completeness: {{email: .nan}}\n"),
            "data_quality.completeness.email",
        ),
        (
            format!(
                "{original}\ndata_quality:\n  completeness: {{email: 1.0}}\n  weights:\n    completeness: .inf\n    validity: 1.0\n    freshness: 1.0\n"
            ),
            "data_quality.weights.completeness",
        ),
        (
            format!(
                "{original}\ndata_quality:\n  completeness: {{email: 1.0}}\n  weights:\n    completeness: 1.0\n    validity: -.inf\n    freshness: 1.0\n"
            ),
            "data_quality.weights.validity",
        ),
        (
            format!(
                "{original}\ndata_quality:\n  completeness: {{email: 1.0}}\n  weights:\n    completeness: 1.0\n    validity: 1.0\n    freshness: .nan\n"
            ),
            "data_quality.weights.freshness",
        ),
    ] {
        let error = load_config_str(&yaml).unwrap_err();
        assert!(error.to_string().contains(path), "{path}: {error}");
    }
}

#[test]
#[should_panic(expected = "config contains a nonfinite numeric field")]
fn direct_config_hash_rejects_nonfinite_numbers() {
    let mut config = load_config_str(fixture("default.yaml")).unwrap();
    config.thresholds.auto_merge = f64::NAN;
    let _ = config_hash(&config);
}

#[test]
fn source_names_must_be_record_key_components() {
    let original = fixture("default.yaml");
    for source in ["\"\"", "\"a:b\""] {
        let yaml = original.replace("  crm:\n", &format!("  {source}:\n"));
        let error = load_config_str(&yaml).unwrap_err();
        assert!(
            error.to_string().contains("source name") && error.to_string().contains("':'"),
            "{source}: {error}"
        );
    }
    assert!(load_config_str(original).is_ok());
}
