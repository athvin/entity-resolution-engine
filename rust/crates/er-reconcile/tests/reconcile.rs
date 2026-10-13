//! Byte-exact parity of the INV-PERM reconcile_plan against a golden from the pure
//! Python `er.entities.reconcile.reconcile_plan` (deterministic CountingIdFactory
//! start=0 on both sides). Regenerate with
//! `uv run python rust/crates/er-reconcile/tests/gen_reconcile_golden.py`.

use std::collections::BTreeMap;

use er_ids::CountingIdFactory;
use er_reconcile::reconcile::reconcile_plan;

fn str_vec(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn reconcile_plan_matches_python_golden() {
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("reconcile_golden.json")).unwrap();
    let cases = golden["cases"].as_array().unwrap();
    let count = cases.len();
    assert!(count >= 6, "expected a broad case set");

    for case in cases {
        let name = case["name"].as_str().unwrap();
        let p_old: BTreeMap<String, Vec<String>> = case["p_old"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), str_vec(v)))
            .collect();
        let p_new: Vec<Vec<String>> = case["p_new"]
            .as_array()
            .unwrap()
            .iter()
            .map(str_vec)
            .collect();

        let plan = reconcile_plan(&p_old, &p_new, &CountingIdFactory::new(0));

        // assignments
        let got_assign: Vec<(String, String)> = plan
            .assignments
            .iter()
            .map(|a| (a.record_key.clone(), a.entity_id.clone()))
            .collect();
        let want_assign: Vec<(String, String)> = case["assignments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let pair = p.as_array().unwrap();
                (
                    pair[0].as_str().unwrap().to_string(),
                    pair[1].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(got_assign, want_assign, "{name}: assignments differ");

        // transitions
        let got_trans: Vec<(String, String, Option<String>, bool)> = plan
            .transitions
            .iter()
            .map(|t| {
                (
                    t.entity_id.clone(),
                    t.status.clone(),
                    t.merged_into.clone(),
                    t.is_new,
                )
            })
            .collect();
        let want_trans: Vec<(String, String, Option<String>, bool)> = case["transitions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                (
                    t["entity_id"].as_str().unwrap().to_string(),
                    t["status"].as_str().unwrap().to_string(),
                    t["merged_into"].as_str().map(str::to_string),
                    t["is_new"].as_bool().unwrap(),
                )
            })
            .collect();
        assert_eq!(got_trans, want_trans, "{name}: transitions differ");

        // minted
        assert_eq!(
            plan.minted,
            str_vec(&case["minted"]),
            "{name}: minted differ"
        );
    }
}
