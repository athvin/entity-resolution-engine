//! Byte-exact parity of the address parser against a golden dump from the Python
//! `RegexV1Parser` (the oracle for `dbt/macros/std/address_parse.sql`). Regenerate
//! with `uv run python rust/crates/er-std/tests/gen_golden.py`.

use er_std::address::{AddressComponents, REGEX_V1_VERSION, RegexV1Parser};

#[test]
fn matches_python_regex_v1_golden() {
    let raw = include_str!("address_golden.json");
    let golden: serde_json::Value = serde_json::from_str(raw).unwrap();
    assert_eq!(golden["version"].as_str().unwrap(), REGEX_V1_VERSION);

    let parser = RegexV1Parser;
    let cases = golden["cases"].as_array().unwrap();
    assert!(cases.len() >= 20, "expected a broad case set");

    for case in cases {
        // JSON null -> None (the Python `address_line: str | None`).
        let input = case["input"].as_str();
        let expected = AddressComponents {
            addr_number: case["addr_number"].as_str().map(str::to_string),
            addr_street: case["addr_street"].as_str().map(str::to_string),
            addr_unit: case["addr_unit"].as_str().map(str::to_string),
        };
        let got = parser.parse(input);
        assert_eq!(
            got, expected,
            "input {input:?} parsed differently from Python"
        );
    }
}
