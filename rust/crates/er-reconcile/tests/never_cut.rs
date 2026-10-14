//! Byte-exact parity of the never-cut fixpoint against a golden from the pure
//! Python `er.review.never_cut.never_cut_fixpoint`. Regenerate with
//! `uv run python rust/crates/er-reconcile/tests/gen_never_cut_golden.py`.

use er_reconcile::assertions::{Assertion, AssertionKind};
use er_reconcile::never_cut::{PathEdge, never_cut_fixpoint};

fn canon(a: &str, b: &str) -> (String, String) {
    if a < b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

#[test]
fn never_cut_fixpoint_matches_python_golden() {
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("never_cut_golden.json")).unwrap();
    let cases = golden["cases"].as_array().unwrap();
    let count = cases.len();
    assert!(count >= 5, "expected a broad case set");

    for case in cases {
        let name = case["name"].as_str().unwrap();
        let edges: Vec<PathEdge> = case["edges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                let t = e.as_array().unwrap();
                (
                    t[0].as_str().unwrap().to_string(),
                    t[1].as_str().unwrap().to_string(),
                    t[2].as_f64().unwrap(),
                )
            })
            .collect();
        let assertions: Vec<Assertion> = case["assertions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| {
                let t = a.as_array().unwrap();
                let (lo, hi) = canon(t[0].as_str().unwrap(), t[1].as_str().unwrap());
                Assertion {
                    assertion_id: t[2].as_str().unwrap().to_string(),
                    rec_a_key: lo,
                    rec_b_key: hi,
                    kind: AssertionKind::Never,
                    active: true,
                }
            })
            .collect();
        let cut_protect = case["cut_protect"].as_f64().unwrap();

        let result = never_cut_fixpoint(&edges, &assertions, &[], cut_protect, 50)
            .unwrap_or_else(|e| panic!("{name}: {e}"));

        let got_cuts: Vec<(String, String, f64, String)> = result
            .cuts
            .iter()
            .map(|c| {
                (
                    c.rec_a_key.clone(),
                    c.rec_b_key.clone(),
                    c.match_probability,
                    c.assertion_id.clone(),
                )
            })
            .collect();
        let want_cuts: Vec<(String, String, f64, String)> = case["cuts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                let t = c.as_array().unwrap();
                (
                    t[0].as_str().unwrap().to_string(),
                    t[1].as_str().unwrap().to_string(),
                    t[2].as_f64().unwrap(),
                    t[3].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(got_cuts, want_cuts, "{name}: cuts differ");

        let got_esc: Vec<(String, String, String)> = result.escalations.clone();
        let want_esc: Vec<(String, String, String)> = case["escalations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                let t = e.as_array().unwrap();
                (
                    t[0].as_str().unwrap().to_string(),
                    t[1].as_str().unwrap().to_string(),
                    t[2].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(got_esc, want_esc, "{name}: escalations differ");

        assert_eq!(
            result.iterations,
            usize::try_from(case["iterations"].as_u64().unwrap()).unwrap(),
            "{name}: iterations differ",
        );
    }
}
