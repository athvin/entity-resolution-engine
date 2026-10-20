//! The compiler's oracle: compiling the committed tenant config must reproduce
//! the committed model's structure, level for level.
//!
//! This is what makes the compiler trustworthy rather than merely plausible.
//! `fixtures/static/model_test_v1.json` is the document the engine has always
//! executed — EM-trained, golden-gated, the thing `base_10`'s expected outputs
//! were produced by. `er-config/tests/fixtures/test.yaml` is the config that
//! *describes* that same matching, in a DSL nothing read.
//!
//! If compiling the second yields the structure of the first — same comparisons,
//! same levels in the same order, same SQL, same null level, same TF placement —
//! then the DSL was always an accurate description of the model, and the
//! compiler is a faithful reader of it rather than a new dialect.
//!
//! What is deliberately NOT compared is the m/u numbers. Those were estimated by
//! EM from a corpus; a compiled model declares them from weights instead. That
//! is the one real difference between a compiled model and a trained one, and
//! it is why `compile` records it in the model's notes.

use std::collections::BTreeMap;

use dedupe_core::settings::json::{BlockingRuleJson, SettingsJson};
use er_config::load_config_str;

const TEST_YAML: &str = include_str!("../../er-config/tests/fixtures/test.yaml");

fn committed_model() -> SettingsJson {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/static/model_test_v1.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    SettingsJson::from_json_str(&text).expect("the committed model parses")
}

/// Each comparison's levels as `(sql_condition, is_null_level, has_tf)` — the
/// structure, with the learned numbers left out.
fn shape(settings: &SettingsJson) -> BTreeMap<String, Vec<(String, bool, bool)>> {
    settings
        .comparisons
        .iter()
        .map(|c| {
            (
                c.output_column_name.clone(),
                c.comparison_levels
                    .iter()
                    .map(|l| {
                        (
                            l.sql_condition.clone(),
                            l.is_null_level,
                            l.tf_adjustment_column.is_some(),
                        )
                    })
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn compiling_the_committed_config_reproduces_the_committed_models_structure() {
    let config = load_config_str(TEST_YAML).expect("the committed config loads");
    let compiled = er_compile::compile(&config).expect("the committed config compiles");
    let compiled: SettingsJson =
        SettingsJson::from_json_str(&compiled.model_json).expect("the compiler emits valid JSON");

    let want = shape(&committed_model());
    let got = shape(&compiled);

    assert_eq!(
        got.keys().collect::<Vec<_>>(),
        want.keys().collect::<Vec<_>>(),
        "the compiled model must compare the same columns as the committed one"
    );
    for (column, want_levels) in &want {
        let got_levels = &got[column];
        assert_eq!(
            got_levels, want_levels,
            "comparison {column} compiled to a different level structure than the committed \
             model.\n  compiled: {got_levels:#?}\n  committed: {want_levels:#?}"
        );
    }
}

#[test]
fn the_compiled_model_loads_in_the_engine() {
    // Structure is not enough: the document has to be one the engine accepts,
    // because the failure mode otherwise is a run that dies mid-job.
    let config = load_config_str(TEST_YAML).unwrap();
    let compiled = er_compile::compile(&config).unwrap();
    let settings = dedupe_core::Settings::from_json_str(&compiled.model_json)
        .expect("a compiled model must load through the engine's own parser");
    assert_eq!(
        settings.comparisons.len(),
        config.comparisons.len(),
        "every declared comparison must survive into the loaded settings"
    );
}

#[test]
fn the_declared_blocking_rules_are_what_the_compiled_model_blocks_on() {
    let config = load_config_str(TEST_YAML).unwrap();
    let compiled = er_compile::compile(&config).unwrap();
    let compiled = SettingsJson::from_json_str(&compiled.model_json).unwrap();

    // The config declares a blocking KEY (the value to block on); the engine
    // blocks on a join PREDICATE. Each declared key must appear, in order, as
    // the predicate that compares it on both sides — this block was validated at
    // publish and then ignored, so "in order" is the part that now means
    // something.
    let want: Vec<String> = config
        .blocking
        .iter()
        .map(|b| er_compile::blocking_predicate(&b.expr))
        .collect();
    let got: Vec<String> = compiled
        .blocking_rules_to_generate_predictions
        .iter()
        .map(|r| match r {
            BlockingRuleJson::Sql(s) => s.clone(),
            BlockingRuleJson::Detailed { blocking_rule, .. } => blocking_rule.clone(),
        })
        .collect();
    assert_eq!(got, want);
    assert_ne!(
        want.len(),
        0,
        "the committed config declares blocking rules, so this assertion means something"
    );
    // And every one is a real equality predicate, not a bare key expression —
    // the bug that made the first compiled model block on `(email)` as a boolean.
    for predicate in &got {
        assert!(
            predicate.contains(" = ") && predicate.contains("l.") && predicate.contains("r."),
            "blocking rule {predicate:?} is not a two-sided join predicate"
        );
    }
}

#[test]
fn a_compiled_model_says_its_parameters_are_declared_not_learned() {
    let config = load_config_str(TEST_YAML).unwrap();
    let compiled = er_compile::compile(&config).unwrap();
    assert!(
        compiled
            .notes
            .iter()
            .any(|n| n.contains("DECLARED, not estimated")),
        "a reviewer has to be told the numbers were not trained: {:?}",
        compiled.notes
    );
    // And every level is pinned, so a later training pass cannot quietly move
    // this version's numbers underneath it.
    let settings = SettingsJson::from_json_str(&compiled.model_json).unwrap();
    for comparison in &settings.comparisons {
        for level in &comparison.comparison_levels {
            assert!(
                level.fix_m_probability && level.fix_u_probability,
                "level {:?} of {} is not pinned",
                level.sql_condition,
                comparison.output_column_name
            );
        }
    }
}

#[test]
fn equal_weights_give_every_comparison_the_same_pull() {
    // The default: no weights declared, so the score is shared equally. Checked
    // through the compiled document rather than the internals, since that is
    // what the engine reads.
    let config = load_config_str(TEST_YAML).unwrap();
    let compiled = er_compile::compile(&config).unwrap();
    let settings = SettingsJson::from_json_str(&compiled.model_json).unwrap();

    let mut top_bits: Vec<(String, f64)> = Vec::new();
    for comparison in &settings.comparisons {
        // The first non-null level is the field's strongest evidence.
        let top = comparison
            .comparison_levels
            .iter()
            .find(|l| !l.is_null_level)
            .expect("every comparison has a positive level");
        let bits = (top.m_probability.unwrap() / top.u_probability.unwrap()).log2();
        top_bits.push((comparison.output_column_name.clone(), bits));
    }
    let first = top_bits[0].1;
    for (column, bits) in &top_bits {
        assert!(
            (bits - first).abs() < 1e-9,
            "with no weights declared every field should pull equally, but {column} contributes \
             {bits} bits against {first}"
        );
    }
}

#[test]
fn a_declared_weight_changes_how_much_a_field_counts() {
    // Weight family_name three times given_name and check the compiled document
    // reflects it. Written as a YAML edit so it exercises the real config path
    // rather than a hand-built struct — including the `weight` field surviving
    // deserialization of the committed flow-style comparisons block.
    let weighted = TEST_YAML
        .replace(
            "given_name:  { levels: [exact, jaro_winkler:0.90, variant_match, null], tf: true }",
            "given_name:  { levels: [exact, jaro_winkler:0.90, variant_match, null], tf: true, weight: 15 }",
        )
        .replace(
            "family_name: { levels: [exact, jaro_winkler:0.90, null], tf: true }",
            "family_name: { levels: [exact, jaro_winkler:0.90, null], tf: true, weight: 45 }",
        )
        .replace(
            "email:       { levels: [exact, username_exact, null], tf: true }",
            "email:       { levels: [exact, username_exact, null], tf: true, weight: 20 }",
        )
        .replace(
            "phone_e164:  { levels: [exact, null] }",
            "phone_e164:  { levels: [exact, null], weight: 10 }",
        )
        .replace(
            "birth_date:  { levels: [exact, dob_same_year_month, null] }",
            "birth_date:  { levels: [exact, dob_same_year_month, null], weight: 5 }",
        )
        .replace(
            "addr_postal: { levels: [exact, null] }",
            "addr_postal: { levels: [exact, null], weight: 5 }",
        );
    assert_ne!(weighted, TEST_YAML, "the weight edits must apply");

    let config = load_config_str(&weighted)
        .unwrap_or_else(|e| panic!("the weighted config should load: {e:?}"));
    let compiled = er_compile::compile(&config).expect("the weighted config compiles");
    let settings = SettingsJson::from_json_str(&compiled.model_json).unwrap();

    let bits_of = |column: &str| -> f64 {
        let c = settings
            .comparisons
            .iter()
            .find(|c| c.output_column_name == column)
            .unwrap();
        let top = c
            .comparison_levels
            .iter()
            .find(|l| !l.is_null_level)
            .unwrap();
        (top.m_probability.unwrap() / top.u_probability.unwrap()).log2()
    };
    let family = bits_of("family_name");
    let given = bits_of("given_name");
    assert!(
        family > given,
        "family_name at weight 45 must outweigh given_name at 15 ({family} vs {given})"
    );
    // Three times the weight is three times the evidence at full agreement.
    assert!(
        ((family / given) - 3.0).abs() < 1e-6,
        "three times the weight should be three times the evidence: {family} / {given}"
    );
    // And a weighted config no longer reports the equal-share note.
    assert!(
        !compiled
            .notes
            .iter()
            .any(|n| n.contains("share the score equally")),
        "the equal-share note should not appear once weights are declared: {:?}",
        compiled.notes
    );
}

#[test]
fn weights_that_do_not_total_a_hundred_are_refused_with_the_total_named() {
    // One field weighted and the rest left unset: the unweighted fields would
    // otherwise silently default and fall out of the model.
    let partial = TEST_YAML.replace(
        "addr_postal: { levels: [exact, null] }",
        "addr_postal: { levels: [exact, null], weight: 10 }",
    );
    assert_ne!(partial, TEST_YAML, "the weight edit must apply");
    let config = load_config_str(&partial).unwrap();
    let err = er_compile::compile(&config).expect_err("a lone 10% weight cannot be right");
    let message = err.to_string();
    assert!(
        message.contains("10") && message.contains("100"),
        "the refusal should name the declared total and the target: {message}"
    );
}
