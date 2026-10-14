//! Byte-exact parity of CONTRADICTION-1 against a golden from the pure Python
//! `er.review.assertions.check_contradiction_1`. Regenerate with
//! `uv run python rust/crates/er-reconcile/tests/gen_contradiction_golden.py`.

use std::collections::BTreeSet;

use er_reconcile::assertions::{Assertion, AssertionKind, check_contradiction_1};

/// (rec_a_key, rec_b_key, never_assertion_id, always_assertion_ids, component).
type Finding = (String, String, String, Vec<String>, BTreeSet<String>);

fn canon(a: &str, b: &str) -> (String, String) {
    if a < b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

#[test]
fn check_contradiction_1_matches_python_golden() {
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("contradiction_golden.json")).unwrap();
    let cases = golden["cases"].as_array().unwrap();
    let count = cases.len();
    assert!(count >= 5, "expected a broad case set");

    for case in cases {
        let name = case["name"].as_str().unwrap();
        let assertions: Vec<Assertion> = case["assertions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| {
                let t = a.as_array().unwrap();
                let (lo, hi) = canon(t[1].as_str().unwrap(), t[2].as_str().unwrap());
                Assertion {
                    assertion_id: t[3].as_str().unwrap().to_string(),
                    rec_a_key: lo,
                    rec_b_key: hi,
                    kind: if t[0].as_str().unwrap() == "always" {
                        AssertionKind::Always
                    } else {
                        AssertionKind::Never
                    },
                    active: t[4].as_bool().unwrap(),
                }
            })
            .collect();

        let got = check_contradiction_1(&assertions);
        let got_tuples: Vec<Finding> = got
            .iter()
            .map(|c| {
                (
                    c.rec_a_key.clone(),
                    c.rec_b_key.clone(),
                    c.never_assertion_id.clone(),
                    c.always_assertion_ids.clone(),
                    c.component.clone(),
                )
            })
            .collect();

        let want_tuples: Vec<Finding> = case["contradictions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    c["rec_a_key"].as_str().unwrap().to_string(),
                    c["rec_b_key"].as_str().unwrap().to_string(),
                    c["never_assertion_id"].as_str().unwrap().to_string(),
                    c["always_assertion_ids"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|x| x.as_str().unwrap().to_string())
                        .collect(),
                    c["component"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|x| x.as_str().unwrap().to_string())
                        .collect(),
                )
            })
            .collect();

        assert_eq!(got_tuples, want_tuples, "{name}: contradictions differ");
    }
}
