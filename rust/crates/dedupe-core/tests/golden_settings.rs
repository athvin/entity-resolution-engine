//! Settings golden gate: the compiled settings model must agree with the
//! golden introspection of the same model JSON (gamma values, referenced
//! columns, exact-match detection, TF flags), and saved-model JSON must
//! round-trip.

use crate::common;

use std::collections::BTreeSet;

use common::{load_golden_json, load_golden_text};
use dedupe_core::Settings;
use serde_json::Value;

fn as_str_set(v: &Value) -> BTreeSet<String> {
    v.as_array()
        .map(|a| a.iter().map(|s| s.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

#[test]
fn compiled_settings_match_golden_introspection() {
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();
    let intro = load_golden_json("settings_introspection.json");
    let intro_comparisons = intro["comparisons"].as_array().unwrap();

    assert_eq!(settings.comparisons.len(), intro_comparisons.len());

    for (c, ic) in settings.comparisons.iter().zip(intro_comparisons) {
        let name = ic["output_column_name"].as_str().unwrap();
        assert_eq!(c.output_column_name, name);
        assert_eq!(
            c.gamma_column_name(),
            ic["gamma_column_name"].as_str().unwrap(),
            "{name}: gamma column"
        );
        assert_eq!(
            c.mw_column_name(),
            ic["mw_column_name"].as_str().unwrap(),
            "{name}: mw column"
        );
        let expected_cols = as_str_set(&ic["input_columns_used_by_case_statement"]);
        let actual_cols: BTreeSet<String> = c.input_columns.iter().cloned().collect();
        assert_eq!(actual_cols, expected_cols, "{name}: case statement columns");

        let intro_levels = ic["levels"].as_array().unwrap();
        assert_eq!(c.levels.len(), intro_levels.len(), "{name}: level count");

        for (l, il) in c.levels.iter().zip(intro_levels) {
            let cond = il["sql_condition"].as_str().unwrap();
            assert_eq!(l.sql_condition, cond, "{name}: level condition");
            assert_eq!(
                i64::from(l.gamma),
                il["comparison_vector_value"].as_i64().unwrap(),
                "{name} / {cond}: gamma"
            );
            assert_eq!(
                l.is_null_level,
                il["is_null_level"].as_bool().unwrap(),
                "{name} / {cond}: null level"
            );
            assert_eq!(
                l.exact_match_columns.is_some(),
                il["is_exact_match"].as_bool().unwrap(),
                "{name} / {cond}: exact match flag"
            );
            if let Some(cols) = &l.exact_match_columns {
                let actual: BTreeSet<String> = cols.iter().cloned().collect();
                assert_eq!(
                    actual,
                    as_str_set(&il["exact_match_colnames"]),
                    "{name} / {cond}: exact match columns"
                );
            }
            let actual_inputs: BTreeSet<String> = l.input_columns.iter().cloned().collect();
            assert_eq!(
                actual_inputs,
                as_str_set(&il["input_columns_used_by_sql_condition"]),
                "{name} / {cond}: level input columns"
            );
            assert_eq!(
                l.has_tf_adjustments(),
                il["has_tf_adjustments"].as_bool().unwrap(),
                "{name} / {cond}: tf flag"
            );
            let expected_tf = il["tf_adjustment_column"].as_str();
            assert_eq!(
                l.tf.as_ref().map(|t| t.column.as_str()),
                expected_tf,
                "{name} / {cond}: tf column"
            );
        }
    }
}

/// Fill in the model format's serialization defaults so semantically-equal JSON compares
/// equal even when one side omits default-valued keys.
fn normalize(v: &mut Value) {
    if let Some(comparisons) = v.get_mut("comparisons").and_then(|c| c.as_array_mut()) {
        for comp in comparisons {
            if let Some(levels) = comp
                .get_mut("comparison_levels")
                .and_then(|l| l.as_array_mut())
            {
                for level in levels {
                    let obj = level.as_object_mut().unwrap();
                    obj.entry("fix_m_probability").or_insert(Value::Bool(false));
                    obj.entry("fix_u_probability").or_insert(Value::Bool(false));
                    obj.entry("is_null_level").or_insert(Value::Bool(false));
                    obj.entry("disable_tf_exact_match_detection")
                        .or_insert(Value::Bool(false));
                }
            }
        }
    }
}

fn assert_round_trip(golden_name: &str) {
    let original_text = load_golden_text(golden_name);
    let settings = Settings::from_json_str(&original_text).unwrap();
    let emitted = serde_json::to_value(settings.to_settings_json()).unwrap();

    let mut original: Value = serde_json::from_str(&original_text).unwrap();
    let mut emitted = emitted;
    normalize(&mut original);
    normalize(&mut emitted);
    assert_eq!(emitted, original, "{golden_name}: JSON round-trip drifted");

    // Re-compiling the emitted JSON must produce an identical compiled model.
    let recompiled =
        Settings::from_json_str(&serde_json::to_string(&settings.to_settings_json()).unwrap())
            .unwrap();
    assert_eq!(recompiled.comparisons, settings.comparisons);
    assert_eq!(recompiled.blocking_rules, settings.blocking_rules);
    assert_eq!(recompiled.prior, settings.prior);
}

#[test]
fn saved_model_round_trips() {
    assert_round_trip("model_final.json");
}

#[test]
fn untrained_model_round_trips() {
    assert_round_trip("model_initial.json");
}

#[test]
fn the_comparator_algorithm_version_round_trips_and_absent_reads_as_one() {
    // A stamped model keeps its stamp across the parse → compile → re-emit
    // round-trip that training (`er train`) saves through.
    let mut doc = load_golden_json("model_final.json");
    doc["comparator_algorithm_version"] = Value::from(1u64);
    let text = serde_json::to_string(&doc).unwrap();
    let settings = Settings::from_json_str(&text).unwrap();
    assert_eq!(settings.json.comparator_algorithm_version(), 1);
    let emitted = serde_json::to_value(settings.to_settings_json()).unwrap();
    assert_eq!(emitted["comparator_algorithm_version"], Value::from(1u64));

    // A pre-versioning document carries no stamp and still reads as 1 —
    // every such model was built by the v1 implementation — and it re-emits
    // WITHOUT the key, so a legacy document stays byte-identical.
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();
    assert_eq!(settings.json.comparator_algorithm_version(), 1);
    let emitted = serde_json::to_value(settings.to_settings_json()).unwrap();
    assert!(
        emitted.get("comparator_algorithm_version").is_none(),
        "an unstamped model must not acquire a stamp by round-tripping"
    );
}

#[test]
fn trained_models_after_each_step_compile() {
    for name in [
        "model_after_prior.json",
        "model_after_u.json",
        "model_after_em1.json",
        "model_after_u_simple.json",
        "model_after_u_sampled.json",
    ] {
        let s = Settings::from_json_str(&load_golden_text(name)).unwrap();
        assert_eq!(s.comparisons.len(), 5, "{name}");
    }
}

/// How loud the trained model is at full agreement, and each field's share.
///
/// `er-compile` cannot measure this — it builds a model from a config with no
/// training behind it — so it assumes one global constant, 60 bits, calibrated
/// on the committed person reference model. This corpus's own trained model
/// supports about 31, which is the point of measuring rather than assuming:
/// the right scale is a property of the data.
#[test]
fn full_agreement_bits_measures_the_trained_scale() {
    let model = dedupe_core::model::TrainedModel::new(
        Settings::from_json_str(&load_golden_text("model_final.json")).unwrap(),
    );
    let agreement = model.full_agreement_bits();

    assert_eq!(
        agreement.per_field.len(),
        5,
        "one entry per comparison: {:?}",
        agreement.per_field
    );
    // The strongest level of each field is its exact match, which is what makes
    // this the same quantity `DEFAULT_FULL_AGREEMENT_BITS` was calibrated from.
    for field in &agreement.per_field {
        assert!(
            field.label_for_charts.starts_with("Exact match on"),
            "{}: strongest level is {:?}",
            field.comparison_name,
            field.label_for_charts
        );
        assert!(
            field.bits > 0.0,
            "{} contributes {} bits, which would mean agreeing on it is evidence AGAINST a \
             match",
            field.comparison_name,
            field.bits
        );
    }

    // The total is the sum and nothing else — asserted because a mean or a max
    // would also look plausible in the report, and only a sum is the evidence
    // the scorer accumulates.
    let summed: f64 = agreement.per_field.iter().map(|f| f.bits).sum();
    assert!(
        (agreement.total_bits - summed).abs() < 1e-12,
        "total {} vs summed {summed}",
        agreement.total_bits
    );
    // Pinned loosely: this moves if the goldens are retrained, and the value
    // worth protecting is the order of magnitude, not the decimals.
    assert!(
        (agreement.total_bits - 31.0).abs() < 1.0,
        "this corpus has measured ~31.0 bits; got {:.3} ({:?})",
        agreement.total_bits,
        agreement
            .per_field
            .iter()
            .map(|f| format!("{} {:.2}", f.comparison_name, f.bits))
            .collect::<Vec<_>>()
    );
}
