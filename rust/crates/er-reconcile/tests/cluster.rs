//! Byte-exact parity of label propagation against a golden from the Python
//! `er.entities.cluster.label_propagate`. Regenerate with
//! `uv run python rust/crates/er-reconcile/tests/gen_cluster_golden.py`.

use std::collections::BTreeMap;

use er_reconcile::cluster::label_propagate;

#[test]
fn label_propagate_matches_python_golden() {
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("cluster_golden.json")).unwrap();
    let cases = golden["cases"].as_array().unwrap();
    let count = cases.len();
    assert!(count >= 8, "expected a broad case set");

    for case in cases {
        let name = case["name"].as_str().unwrap();
        let nodes: Vec<String> = case["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let edges: Vec<(String, String)> = case["edges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                let pair = e.as_array().unwrap();
                (
                    pair[0].as_str().unwrap().to_string(),
                    pair[1].as_str().unwrap().to_string(),
                )
            })
            .collect();
        let expected: BTreeMap<String, String> = case["labels"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect();

        assert_eq!(
            label_propagate(&nodes, &edges),
            expected,
            "label_propagate case {name} differs from Python",
        );
    }
}
