//! The 1.9 golden: a phone carried in `mobile` on record A matches the same
//! phone carried in `home_phone` on record B — the pair is GENERATED (the
//! group's blocking rule is the only one in this model, so presence in the
//! predictions is proof) and SCORED as a match.
//!
//! Run through the real engine (`dedupe_core::predict::run_predict` over an
//! in-memory session), not by eyeballing the compiled SQL: a group whose level
//! SQL was valid but meant the wrong thing would pass a string assertion and
//! fail here. Gated behind `DEDUPE_E2E_EXTENSIONS` like the other tests that
//! need a real engine session.

use dedupe_core::Settings;
use dedupe_core::predict;
use dedupe_core::session::{Session, SessionOptions};

fn enabled() -> bool {
    std::env::var("DEDUPE_E2E_EXTENSIONS").as_deref() == Ok("1")
        || std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() == Ok("1")
}

/// A config whose whole model is the one group: no per-column comparisons, no
/// declared blocking rules. That isolation is what makes the assertions below
/// load-bearing — nothing else can generate or score a pair.
fn group_only_settings() -> Settings {
    let yaml = format!(
        "{}\ncomparison_groups:\n  any_phone:\n    attributes: [mobile, home_phone]\n    levels: [exact]\n",
        include_str!("../../er-config/tests/fixtures/test.yaml")
    );
    let mut config = er_config::load_config_str(&yaml).unwrap();
    config.comparisons.clear();
    config.blocking.clear();
    let compiled = er_compile::compile(&config).expect("the group-only config compiles");
    Settings::from_json_str(&compiled.model_json).expect("the compiled model parses")
}

#[test]
fn cross_field_phone_agreement_generates_and_scores_the_pair() {
    if !enabled() {
        eprintln!("skipping cross-field group behaviour (set DEDUPE_E2E_EXTENSIONS=1)");
        return;
    }
    let settings = group_only_settings();
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    session
        .execute_batch(
            "create table records(record_key varchar, mobile varchar, home_phone varchar);
             insert into records values
               ('rk1', '+14155550100', null),
               ('rk2', null, '+14155550100'),
               ('rk3', '+15125550199', null)",
        )
        .unwrap();
    let input = session.register_table("records", "records").unwrap();

    let predictions = predict::run_predict(&session, &settings, &input, None).unwrap();
    let rows = session
        .query_strings(&format!(
            "select record_key_l, record_key_r, match_probability from {}",
            predictions.physical
        ))
        .unwrap();

    // The cross-field pair is the ONLY pair: (rk1, rk3) and (rk2, rk3) share
    // no phone under any member pairing, and the group's rule is the only
    // blocking rule — so the pair's presence proves generation, not merely
    // scoring.
    assert_eq!(
        rows.len(),
        1,
        "expected exactly the cross-field pair: {rows:?}"
    );
    assert_eq!(rows[0][0], "rk1");
    assert_eq!(rows[0][1], "rk2");
    let probability: f64 = rows[0][2].parse().unwrap();
    assert!(
        probability > 0.95,
        "phone in mobile on A against home_phone on B must score as a match, got {probability}"
    );
}

#[test]
fn a_record_with_no_member_populated_blocks_with_nobody() {
    if !enabled() {
        eprintln!("skipping cross-field group behaviour (set DEDUPE_E2E_EXTENSIONS=1)");
        return;
    }
    let settings = group_only_settings();
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    session
        .execute_batch(
            "create table records(record_key varchar, mobile varchar, home_phone varchar);
             insert into records values
               ('rk1', '+14155550100', null),
               ('rk4', null, null)",
        )
        .unwrap();
    let input = session.register_table("records", "records").unwrap();

    // The blocking rule is an OR of equalities, and an equality over NULLs is
    // never true — so rk4, which populates no member, is never a candidate.
    // This is the pool's null semantics reaching generation, not only scoring.
    let predictions = predict::run_predict(&session, &settings, &input, None).unwrap();
    let rows = session
        .query_strings(&format!(
            "select record_key_l from {}",
            predictions.physical
        ))
        .unwrap();
    assert!(rows.is_empty(), "expected no candidates, got {rows:?}");
}
