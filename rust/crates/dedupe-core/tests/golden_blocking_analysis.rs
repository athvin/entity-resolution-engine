//! Blocking-analysis golden gate: per-rule comparison counts (full sample,
//! marginal + cumulative) must match the committed goldens.

mod common;

use common::{FAKE_1000_SCHEMA, fake_1000_csv_path, load_golden_json, load_golden_text};
use dedupe_core::Settings;
use dedupe_core::blocking_analysis::count_comparisons_from_blocking_rules;
use dedupe_core::predict;
use dedupe_core::session::{Session, SessionOptions};

#[test]
fn blocking_counts_match_golden() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = session
        .register_csv(&fake_1000_csv_path(), "fake_1000", FAKE_1000_SCHEMA)
        .unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();
    let concat = predict::materialize_concat(&session, &input).unwrap();

    let rules = vec![
        r#"(l."first_name" = r."first_name") AND (l."city" = r."city")"#.to_string(),
        r#"l."surname" = r."surname""#.to_string(),
    ];
    let counts =
        count_comparisons_from_blocking_rules(&session, &settings, &concat, &rules).unwrap();

    let golden = load_golden_json("blocking_analysis.json");
    let golden = golden.as_array().unwrap();
    assert_eq!(counts.len(), golden.len());
    for (c, g) in counts.iter().zip(golden) {
        assert_eq!(c.blocking_rule, g["blocking_rule"].as_str().unwrap());
        assert_eq!(c.match_key, g["match_key"].as_i64().unwrap(), "match_key");
        assert_eq!(
            c.marginal_comparison_count,
            g["marginal_comparison_count"].as_i64().unwrap(),
            "marginal for rule {}",
            c.match_key
        );
        assert_eq!(
            c.cumulative_comparison_count,
            g["cumulative_comparison_count"].as_i64().unwrap(),
            "cumulative for rule {}",
            c.match_key
        );
        assert_eq!(
            c.total_possible_comparison_count,
            g["total_possible_comparison_count"].as_i64().unwrap(),
            "total for rule {}",
            c.match_key
        );
    }
}
