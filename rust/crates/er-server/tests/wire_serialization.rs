//! #868: no handler serializes a whole value straight onto a client path.
//!
//! `Json(json!(row))` is the shape that leaks: it serializes every field the
//! struct has, so an entity row carries its internal columns (`key_hash`,
//! `token_hash`, a webhook `secret`) onto the wire, and nothing in the type
//! system notices. Since #864 every success body is a named struct or a
//! literal with its keys spelled out, and entities cross through their `*Wire`
//! form, which omits non-public columns by construction. This pins that: the
//! wholesale shape may not come back in the handler source.
//!
//! A source scan rather than a type check because the leak is exactly the
//! case the types allow (`json!` accepts anything `Serialize`).

const LIB: &str = include_str!("../src/lib.rs");

/// The source with every `#[cfg(test)] mod … { … }` block removed, so test
/// code that compares `json!(entity)` against its wire form does not count.
pub(crate) fn product_source(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(at) = rest.find("#[cfg(test)]\nmod ") {
        out.push_str(&rest[..at]);
        let block = &rest[at..];
        let Some(open) = block.find('{') else {
            break;
        };
        let mut depth = 0usize;
        let mut end = block.len();
        for (i, c) in block[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        rest = &block[end..];
    }
    out.push_str(rest);
    out
}

/// Every `Json(json!(<path>))` in `src`: a bare value serialized wholesale.
fn wholesale(src: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = src;
    while let Some(at) = rest.find("Json(json!(") {
        let after = &rest[at + "Json(json!(".len()..];
        let ident: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
            .collect();
        let tail = after[ident.len()..].trim_start();
        if !ident.is_empty() && tail.starts_with(')') {
            found.push(format!("Json(json!({ident}))"));
        }
        rest = after;
    }
    found
}

#[test]
fn the_detector_sees_the_wholesale_shape_and_only_it() {
    assert_eq!(
        wholesale("Ok(rows) => Json(json!(rows)).into_response(),"),
        ["Json(json!(rows))"]
    );
    assert_eq!(
        wholesale("Json(json!(self.row))"),
        ["Json(json!(self.row))"]
    );
    // Spelled-out keys and a detail body are fine.
    assert_eq!(
        wholesale(r#"Json(json!({ "items": items }))"#),
        Vec::<String>::new()
    );
    assert_eq!(
        wholesale(r#"Json(json!({ "detail": msg }))"#),
        Vec::<String>::new()
    );
    // A test module is not product code.
    let src = "fn h() {}\n#[cfg(test)]\nmod tests {\n    fn t() { Json(json!(job)); }\n}\n";
    assert_eq!(wholesale(&product_source(src)), Vec::<String>::new());
    assert_eq!(wholesale(src).len(), 1);
}

#[test]
fn no_handler_serializes_a_bare_value_onto_the_wire() {
    let product = product_source(LIB);
    // A floor, so a source that stopped being read cannot pass vacuously.
    assert!(
        product.contains("async fn create_key("),
        "lib.rs was not read"
    );
    assert_eq!(
        wholesale(&product),
        Vec::<String>::new(),
        "serialize a named response struct or an entity's *Wire form instead"
    );
}
