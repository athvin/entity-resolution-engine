//! The score scale is one global constant, and the two committed reference
//! models disagree sharply about what it should be.
//!
//! `DEFAULT_FULL_AGREEMENT_BITS = 60.0` is not a guess — its doc comment records
//! the derivation: "summing the committed trained model's own exact-level Bayes
//! factors across its six comparisons comes to roughly 60 bits", confirmed by a
//! grid search over `base_10` where 20 bits under-merges (18 clusters where
//! there are 10) and 120 bits over-merges (9). That derivation is sound, and the
//! first test here reproduces it: the committed **person** model measures 59.9.
//!
//! The second test is the finding. The committed **account** model measures
//! **21.8** bits over its four comparisons, and `compile` hands every config the
//! same 60 — `compile_with` never consults `config.entity_type`, and the only
//! production entry point is `compile`, which passes `CompileOptions::default()`.
//! So an account config is compiled roughly 2.7x louder than its own reference
//! model, in the direction the constant's own doc comment calls the more
//! expensive mistake: "louder is not safer. Over-weighting trades missed
//! duplicates for false merges."
//!
//! This is deliberately a *measurement*, not a fix. The person number earned its
//! value from two independent legs — the reference-model sum AND a cluster-count
//! grid search — and only the first leg can be run for accounts: there is no
//! account config fixture and no account corpus with a known cluster count, so
//! re-tuning the constant to 22 would swap one uncalibrated number for another
//! and move every account tenant's scoring. Recording the gap is what this file
//! is for; closing it is a decision with evidence attached.

use dedupe_core::Settings;
use dedupe_core::model::TrainedModel;
use er_compile::CompileOptions;
use er_config::load_config_str;

const TEST_YAML: &str = include_str!("../../er-config/tests/fixtures/test.yaml");

/// The one committed account config. It lives under `er-standardize` rather than
/// beside the person fixture, which is a large part of why the compile tests had
/// no account coverage until one went looking for it.
const ACCOUNT_YAML: &str = include_str!("../../er-standardize/tests/fixtures/acct_test.yaml");

/// Load one of the committed reference models from `fixtures/static/`.
fn committed_model(name: &str) -> TrainedModel {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join("fixtures/static")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    TrainedModel::new(
        Settings::from_json_str(&text)
            .unwrap_or_else(|e| panic!("parsing {}: {e}", path.display())),
    )
}

/// The full-agreement bits of whatever `compile` produces for this config.
fn compiled_bits(yaml: &str) -> f64 {
    let config = load_config_str(yaml).expect("the config parses");
    let compiled = er_compile::compile(&config).expect("the config compiles");
    let settings = Settings::from_json_str(&compiled.model_json).expect("the output parses");
    TrainedModel::new(settings).full_agreement_bits().total_bits
}

#[test]
fn the_person_constant_is_what_the_person_model_measures() {
    let model = committed_model("model_test_v1.json");
    let agreement = model.full_agreement_bits();
    let assumed = CompileOptions::default().full_agreement_bits;

    assert_eq!(
        agreement.per_field.len(),
        6,
        "the person reference model has six comparisons: {:?}",
        agreement.per_field
    );
    // Every field's strongest level is its exact match, which is what makes
    // "exact-level Bayes factors" in the constant's doc comment the same
    // quantity this measures.
    for field in &agreement.per_field {
        assert!(
            field.label_for_charts.starts_with("Exact match on"),
            "the strongest level should be the exact one, got {:?} for {}",
            field.label_for_charts,
            field.comparison_name
        );
    }
    assert!(
        (agreement.total_bits - assumed).abs() < 1.0,
        "the constant is supposed to BE this measurement: model {:.3} bits vs assumed \
         {assumed:.1}",
        agreement.total_bits
    );
}

#[test]
fn the_account_model_measures_far_less_than_the_compiler_assumes() {
    let account = committed_model("acct_model_v1.json").full_agreement_bits();
    let person = committed_model("model_test_v1.json").full_agreement_bits();
    let assumed = CompileOptions::default().full_agreement_bits;

    assert_eq!(
        account.per_field.len(),
        4,
        "the account reference model has four comparisons: {:?}",
        account.per_field
    );

    // The assertion is a bound rather than a point, so this does not have to be
    // re-tuned every time the reference model is retrained — it fails only if
    // the gap closes (good news, and worth being told about) or inverts.
    assert!(
        account.total_bits < assumed / 2.0,
        "the finding this file records is that the account model is far quieter than the \
         assumption: account {:.3} bits vs assumed {assumed:.1}. If this now passes \
         comfortably, re-read the module comment — the gap may have been closed on purpose, \
         in which case delete this test rather than loosening it.",
        account.total_bits
    );
    assert!(
        account.total_bits < person.total_bits,
        "...and quieter than the person model the constant was calibrated on: account {:.3} \
         vs person {:.3}",
        account.total_bits,
        person.total_bits
    );
}

#[test]
fn the_compiled_scale_does_not_vary_with_entity_type() {
    // The same config compiled as a person and as an account. Nothing else
    // changes, so an identical total proves the scale is entity-blind — which is
    // the mechanism behind the gap the test above measures.
    assert_eq!(
        TEST_YAML.matches("\nentity_type:").count(),
        0,
        "the fixture is a person config by omission; this test adds the key itself"
    );
    let as_person = compiled_bits(TEST_YAML);
    let as_account = compiled_bits(&format!("entity_type: account\n{TEST_YAML}"));

    assert!(
        (as_person - as_account).abs() < f64::EPSILON,
        "the compiled scale must be shown to ignore entity_type for the gap to be real: \
         person {as_person:.6} vs account {as_account:.6}"
    );
    // And it is the constant itself, not some derived fraction of it: the
    // strongest level of each equally-weighted field earns the field's whole
    // share, so the shares sum back to the constant.
    let assumed = CompileOptions::default().full_agreement_bits;
    assert!(
        (as_person - assumed).abs() < 1e-6,
        "a compiled model's full agreement IS `full_agreement_bits`: {as_person:.6} vs \
         {assumed:.1}"
    );
}

#[test]
fn a_real_account_config_compiles_and_inherits_the_person_scale() {
    // The compile tests had NO account coverage at all. That matters twice: the
    // account path could break without anyone noticing, and the gap this file
    // records — the account reference model measuring a third of the assumed
    // scale — deserves to be demonstrated against a genuine account config
    // rather than a person config with `entity_type` flipped, which is all
    // `the_compiled_scale_does_not_vary_with_entity_type` can show.
    let config = load_config_str(ACCOUNT_YAML).expect("the account fixture parses");
    assert_eq!(
        config.entity_type,
        er_config::EntityType::Account,
        "this test is pointless unless the fixture really is an account config"
    );

    let bits = compiled_bits(ACCOUNT_YAML);
    let assumed = CompileOptions::default().full_agreement_bits;
    assert!(
        (bits - assumed).abs() < 1e-6,
        "a real account config compiles to the person-calibrated scale: {bits:.6} vs \
         {assumed:.1}"
    );

    // Which is 2.7x the account reference model's own measurement. Spelled out
    // here, on the account path, because this is the number a decision would
    // turn on.
    let measured = committed_model("acct_model_v1.json")
        .full_agreement_bits()
        .total_bits;
    let ratio = bits / measured;
    assert!(
        ratio > 2.0,
        "the compiled account scale should still be far above what the account model \
         measures: compiled {bits:.1}, measured {measured:.3}, ratio {ratio:.2}. If this \
         fails the gap has closed — re-read the module comment before loosening it."
    );
}

#[test]
fn the_compiler_records_the_scale_it_assumed() {
    // Because the assumption is otherwise invisible in the artifact: the saved
    // model carries m/u values with no trace of the number they were derived
    // from, and a reviewer comparing two tenants' models cannot tell that both
    // inherited one global default.
    let config = load_config_str(TEST_YAML).unwrap();
    let compiled = er_compile::compile(&config).unwrap();
    let note = compiled
        .notes
        .iter()
        .find(|n| n.contains("score scale"))
        .unwrap_or_else(|| panic!("the scale must be noted: {:?}", compiled.notes));
    assert!(
        note.contains("60 bits") && note.contains("not a measurement of your data"),
        "the note must name the number AND that it is an assumption: {note}"
    );
}
