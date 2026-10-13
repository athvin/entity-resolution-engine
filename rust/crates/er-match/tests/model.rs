//! The committed Splink settings model (fixtures/static/model_test_v1.json) must
//! load through dedupe-core's Settings — scenario matching uses it verbatim
//! (never trains). This confirms the model-load half of the matching stage before
//! the frozen-TF predict path is wired.

#[test]
fn committed_model_round_trips() {
    let parsed = er_match::load_model(include_str!("fixtures/model_test_v1.json"));
    assert!(
        parsed.is_ok(),
        "model_test_v1.json failed to parse: {:?}",
        parsed.err()
    );
}
