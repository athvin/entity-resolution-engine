//! The 1.25 golden: records on one ancestor chain of a declared hierarchy are
//! never candidates — parent↔child and grandparent↔grandchild stay apart —
//! while siblings sharing a parent still group. The two directions pull
//! against each other, which is why both are asserted on the same corpus: a
//! "fix" that excludes the whole tree keeps the first assertion green and
//! turns the second red.
//!
//! The compile-shape tests run everywhere; the behaviour tests run the real
//! engine (`dedupe_core::predict` over an in-memory session) and are gated
//! behind `DEDUPE_E2E_EXTENSIONS` like the other tests that need one.

use dedupe_core::Settings;
use dedupe_core::incremental::predict_between;
use dedupe_core::predict;
use dedupe_core::session::{Session, SessionOptions};
use er_config::HierarchyExclusion;

fn enabled() -> bool {
    std::env::var("DEDUPE_E2E_EXTENSIONS").as_deref() == Ok("1")
        || std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() == Ok("1")
}

/// One comparison, one blocking rule, nothing else — so every admitted pair of
/// the shared-phone corpus below is a candidate, and absence from the
/// predictions is the exclusion's doing and nothing else's.
fn base_config() -> er_config::Config {
    let mut config =
        er_config::load_config_str(include_str!("../../er-config/tests/fixtures/test.yaml"))
            .unwrap();
    config.comparisons.clear();
    config.comparison_groups.clear();
    config.blocking.clear();
    config.comparisons.insert(
        "phone_e164".to_string(),
        er_config::ComparisonSpec {
            levels: vec![Some("exact".to_string())],
            tf: false,
            weight: None,
            null_policy: er_config::NullPolicy::default(),
        },
    );
    config.blocking = vec![er_config::BlockingRule {
        key_type: "phone".to_string(),
        expr: "phone_e164".to_string(),
    }];
    config
}

fn hierarchy_config(include_parent_child: bool) -> er_config::Config {
    let mut config = base_config();
    config.constraints.exclude_hierarchy = Some(HierarchyExclusion {
        parent: "ParentId".to_string(),
        include_parent_child,
    });
    config
}

// ---- compile shape ---------------------------------------------------------

#[test]
fn the_conjunct_and_the_settings_block_travel_together() {
    let compiled = er_compile::compile(&hierarchy_config(false)).unwrap();
    let json: serde_json::Value = serde_json::from_str(&compiled.model_json).unwrap();
    let rules = json["blocking_rules_to_generate_predictions"]
        .as_array()
        .unwrap();
    assert_eq!(rules.len(), 1, "the fixture declares exactly one rule");
    for rule in rules {
        assert!(
            rule["blocking_rule"]
                .as_str()
                .unwrap()
                .contains("__dedupe__hierarchy_ancestors"),
            "every rule carries the exclusion conjunct: {rule}"
        );
    }
    assert_eq!(json["hierarchy"]["parent_metadata_key"], "ParentId");
    assert_eq!(json["hierarchy"]["max_depth"], 10);
    // The compiled document round-trips through the engine's own parser.
    assert!(
        Settings::from_json_str(&compiled.model_json)
            .unwrap()
            .json
            .hierarchy
            .is_some()
    );
}

/// The spec's "Include Parent/Child" toggle: the hierarchy stays declared in
/// the CONFIG, but the model carries neither the conjunct nor the block — one
/// without the other would be a model that fails its first blocking pass.
#[test]
fn include_parent_child_removes_both_halves() {
    let compiled = er_compile::compile(&hierarchy_config(true)).unwrap();
    assert!(
        !compiled
            .model_json
            .contains("__dedupe__hierarchy_ancestors")
    );
    assert!(!compiled.model_json.contains("\"hierarchy\""));
    // And an undeclared hierarchy compiles exactly as before.
    let plain = er_compile::compile(&base_config()).unwrap();
    assert_eq!(plain.model_json, compiled.model_json);
}

#[test]
fn a_parent_key_outside_the_identifier_charset_is_refused() {
    let mut config = base_config();
    for bad in ["", "Parent Id", "a'b", "a\"b", "a$.b"] {
        config.constraints.exclude_hierarchy = Some(HierarchyExclusion {
            parent: bad.to_string(),
            include_parent_child: false,
        });
        let err = er_compile::compile(&config).expect_err(bad);
        assert!(
            matches!(err, er_compile::CompileError::BadHierarchyParent { .. }),
            "{bad:?} → {err}"
        );
    }
}

// ---- behaviour, through the real engine -------------------------------------

/// A, with children B and C, and grandchild D under B — all four sharing one
/// phone so the blocking rule generates every pair the exclusion admits.
fn corpus(session: &Session) {
    session
        .execute_batch(
            "create table records(
                 record_key varchar, phone_e164 varchar, source_system varchar,
                 source_record_id varchar, metadata json);
             insert into records values
               ('rk_a', '+14155550100', 'sfdc', '001A', '{}'),
               ('rk_b', '+14155550100', 'sfdc', '001B', '{\"ParentId\": \"001A\"}'),
               ('rk_c', '+14155550100', 'sfdc', '001C', '{\"ParentId\": \"001A\"}'),
               ('rk_d', '+14155550100', 'sfdc', '001D', '{\"ParentId\": \"001B\"}')",
        )
        .unwrap();
}

fn predicted_pairs(settings: &Settings, session: &Session) -> Vec<(String, String)> {
    let input = session.register_table("records", "records").unwrap();
    let predictions = predict::run_predict(session, settings, &input, None).unwrap();
    let rows = session
        .query_strings(&format!(
            "select record_key_l || '|' || record_key_r from {} order by 1",
            predictions.physical
        ))
        .unwrap();
    rows.into_iter()
        .map(|r| {
            let (l, right) = r[0].split_once('|').unwrap();
            (l.to_string(), right.to_string())
        })
        .collect()
}

#[test]
fn ancestor_chains_never_pair_and_siblings_still_do() {
    if !enabled() {
        eprintln!("skipping hierarchy exclusion behaviour (set DEDUPE_E2E_EXTENSIONS=1)");
        return;
    }
    let settings = Settings::from_json_str(
        &er_compile::compile(&hierarchy_config(false))
            .unwrap()
            .model_json,
    )
    .unwrap();
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    corpus(&session);
    let pairs = predicted_pairs(&settings, &session);
    // Of the six possible pairs, the four on ancestor chains (A-B, A-C, B-D,
    // and A-D through the CLOSURE — no single column equates a grandparent
    // with a grandchild) are gone; the sibling pair B-C and the uncle-nephew
    // pair C-D (same tree, no chain between them) survive.
    assert_eq!(
        pairs,
        vec![
            ("rk_b".to_string(), "rk_c".to_string()),
            ("rk_c".to_string(), "rk_d".to_string()),
        ],
        "expected exactly the sibling and uncle-nephew pairs"
    );
}

#[test]
fn the_toggle_admits_the_chains_again() {
    if !enabled() {
        eprintln!("skipping hierarchy toggle behaviour (set DEDUPE_E2E_EXTENSIONS=1)");
        return;
    }
    let settings = Settings::from_json_str(
        &er_compile::compile(&hierarchy_config(true))
            .unwrap()
            .model_json,
    )
    .unwrap();
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    corpus(&session);
    assert_eq!(
        predicted_pairs(&settings, &session).len(),
        6,
        "include_parent_child admits all six pairs of the four-record clique"
    );
}

/// The incremental seam: a NEW child arriving under an EXISTING parent is
/// exactly the pair the closure must still see, so it is built over the union
/// of both sides (`materialize_hierarchy_ancestors_cross`).
#[test]
fn a_new_child_of_an_existing_parent_is_excluded_incrementally() {
    if !enabled() {
        eprintln!("skipping incremental hierarchy behaviour (set DEDUPE_E2E_EXTENSIONS=1)");
        return;
    }
    let settings = Settings::from_json_str(
        &er_compile::compile(&hierarchy_config(false))
            .unwrap()
            .model_json,
    )
    .unwrap();
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    session
        .execute_batch(
            "create table existing(
                 record_key varchar, phone_e164 varchar, source_system varchar,
                 source_record_id varchar, metadata json);
             insert into existing values
               ('rk_a', '+14155550100', 'sfdc', '001A', '{}'),
               ('rk_x', '+14155550100', 'sfdc', '001X', '{}');
             create table new_batch(
                 record_key varchar, phone_e164 varchar, source_system varchar,
                 source_record_id varchar, metadata json);
             insert into new_batch values
               ('rk_d', '+14155550100', 'sfdc', '001D', '{\"ParentId\": \"001A\"}')",
        )
        .unwrap();
    let existing_input = session.register_table("existing", "existing").unwrap();
    let new_input = session.register_table("new_batch", "new_batch").unwrap();
    let existing_concat = predict::materialize_concat(&session, &existing_input).unwrap();
    let new_concat = predict::materialize_concat(&session, &new_input).unwrap();

    let between = predict_between(
        &session,
        &settings,
        &existing_concat,
        &new_concat,
        &[],
        None,
    )
    .unwrap();
    let rows = session
        .query_strings(&format!(
            "select record_key_l || '|' || record_key_r from {} order by 1",
            between.physical
        ))
        .unwrap();
    let pairs: Vec<String> = rows.into_iter().map(|r| r[0].clone()).collect();
    assert_eq!(
        pairs,
        vec!["rk_x|rk_d".to_string()],
        "the unrelated existing record pairs with the new one; its parent does not"
    );
}
