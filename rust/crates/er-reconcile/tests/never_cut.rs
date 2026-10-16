//! Byte-exact parity of the never-cut fixpoint against a golden from the pure
//! Python `er.review.never_cut.never_cut_fixpoint`. Regenerate with
//! `uv run python rust/crates/er-reconcile/tests/gen_never_cut_golden.py`.

use std::collections::BTreeSet;

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
        // `protected` (PR 1.3c) is absent in every Python-parity case ⇒ an empty
        // set ⇒ byte-identical to the pre-1.3c fixpoint. The protection behaviour
        // itself has no Python equivalent and is covered by the Rust-only test below.
        let protected: BTreeSet<String> = case
            .get("protected")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().map(|k| k.as_str().unwrap().to_string()).collect())
            .unwrap_or_default();

        let result = never_cut_fixpoint(&edges, &assertions, &[], cut_protect, &protected, 50)
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

/// PR 1.3c: a protected record is never-cut. On the path a—b—c with a `never`
/// rule on (a, c) and a high `cut_protect` so neither 0.9 edge is protected by
/// probability, the fixpoint normally cuts the lexicographically-first minimum
/// edge (a—b). When `b` is protected, every edge on the only path touches `b`
/// and is therefore off-limits, so the pair escalates instead of severing `b`.
/// An empty protected set must reproduce the pre-1.3c cut exactly.
#[test]
fn protected_record_escalates_instead_of_being_cut() {
    let edges: Vec<PathEdge> = vec![
        ("a".to_string(), "b".to_string(), 0.9),
        ("b".to_string(), "c".to_string(), 0.9),
    ];
    let assertions = vec![Assertion {
        assertion_id: "n1".to_string(),
        rec_a_key: "a".to_string(),
        rec_b_key: "c".to_string(),
        kind: AssertionKind::Never,
        active: true,
    }];
    let nodes: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
    let cut_protect = 1.0;

    // Baseline: no protection cuts the minimum lexicographic edge a—b.
    let baseline = never_cut_fixpoint(
        &edges,
        &assertions,
        &nodes,
        cut_protect,
        &BTreeSet::new(),
        50,
    )
    .unwrap();
    assert_eq!(
        baseline
            .cuts
            .iter()
            .map(|c| (c.rec_a_key.as_str(), c.rec_b_key.as_str()))
            .collect::<Vec<_>>(),
        vec![("a", "b")],
        "unprotected run should cut a—b",
    );
    let no_escalations: Vec<(String, String, String)> = vec![];
    assert_eq!(baseline.escalations, no_escalations);

    // Protecting b makes both of its edges uncuttable ⇒ the never pair escalates.
    let protected: BTreeSet<String> = ["b".to_string()].into_iter().collect();
    let got = never_cut_fixpoint(&edges, &assertions, &nodes, cut_protect, &protected, 50).unwrap();
    assert!(
        got.cuts.is_empty(),
        "no edge touching the protected record may be cut, got {:?}",
        got.cuts,
    );
    assert_eq!(
        got.escalations,
        vec![("a".to_string(), "c".to_string(), "n1".to_string())],
        "the unsatisfiable never pair should escalate",
    );
}

/// PR 1.3c: protecting a record that is *not* on the cut path leaves the cut
/// unchanged — protection is scoped to edges touching the protected record.
#[test]
fn protecting_an_offpath_record_does_not_change_the_cut() {
    let edges: Vec<PathEdge> = vec![
        ("a".to_string(), "b".to_string(), 0.9),
        ("b".to_string(), "c".to_string(), 0.9),
    ];
    let assertions = vec![Assertion {
        assertion_id: "n1".to_string(),
        rec_a_key: "a".to_string(),
        rec_b_key: "c".to_string(),
        kind: AssertionKind::Never,
        active: true,
    }];
    let nodes: Vec<String> = vec!["a".into(), "b".into(), "c".into(), "z".into()];
    // `z` is isolated (no edges) and protected — it must not perturb the cut.
    let protected: BTreeSet<String> = ["z".to_string()].into_iter().collect();
    let got = never_cut_fixpoint(&edges, &assertions, &nodes, 1.0, &protected, 50).unwrap();
    assert_eq!(
        got.cuts
            .iter()
            .map(|c| (c.rec_a_key.as_str(), c.rec_b_key.as_str()))
            .collect::<Vec<_>>(),
        vec![("a", "b")],
        "off-path protection must not change the chosen cut",
    );
    let no_escalations: Vec<(String, String, String)> = vec![];
    assert_eq!(got.escalations, no_escalations);
}
