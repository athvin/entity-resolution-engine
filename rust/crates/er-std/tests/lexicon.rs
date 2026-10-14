//! Byte-exact parity of the lexicon canonicalisation + `lexicon_hash` against a
//! golden dump from the Python `er.std.lexicon`. Regenerate with
//! `uv run python rust/crates/er-std/tests/gen_lexicon_golden.py`.

use er_std::lexicon::{canonical_pair, lexicon_hash};

#[test]
fn matches_python_lexicon_golden() {
    let raw = include_str!("lexicon_golden.json");
    let golden: serde_json::Value = serde_json::from_str(raw).unwrap();

    for case in golden["hash_cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let pairs: Vec<(String, String)> = case["pairs"]
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
        assert_eq!(
            lexicon_hash(pairs),
            case["hash"].as_str().unwrap(),
            "lexicon_hash case {name} differs from Python",
        );
    }

    for case in golden["canon_cases"].as_array().unwrap() {
        let a = case["a"].as_str().unwrap();
        let b = case["b"].as_str().unwrap();
        let got = canonical_pair(a, b);
        if case["ok"].as_bool().unwrap() {
            let out = case["out"].as_array().unwrap();
            let expected = (
                out[0].as_str().unwrap().to_string(),
                out[1].as_str().unwrap().to_string(),
            );
            assert_eq!(got.unwrap(), expected, "canonical_pair({a:?},{b:?})");
        } else {
            assert!(
                got.is_err(),
                "canonical_pair({a:?},{b:?}) should be rejected"
            );
        }
    }
}
