//! Operation compatibility supplements the component response-schema checks.
use serde_json::Value;
use std::collections::BTreeSet;

const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

// Resolve local references in schemas, request bodies, parameters and path items.
// Cyclic reference-only chains cannot provide a shape; recursive object schemas
// are handled separately by the ancestor guard in request_required.
fn resolve<'a>(document: &'a Value, mut value: &'a Value) -> &'a Value {
    let mut seen = BTreeSet::new();
    while let Some(reference) = value.get("$ref").and_then(Value::as_str) {
        if !seen.insert(reference) {
            break;
        }
        let Some(target) = reference
            .strip_prefix('#')
            .and_then(|p| document.pointer(p))
        else {
            break;
        };
        value = target;
    }
    value
}

fn path_parameters(document: &Value, path: &Value, operation: &Value) -> BTreeSet<String> {
    [path, operation]
        .into_iter()
        .filter_map(|v| v.get("parameters").and_then(Value::as_array))
        .flatten()
        .map(|p| resolve(document, p))
        .filter(|p| p["in"] == "path")
        .filter_map(|p| p["name"].as_str().map(str::to_owned))
        .collect()
}

pub(super) fn operation_changes(base: &Value, new: &Value) -> Vec<String> {
    let mut breaks = Vec::new();
    let Some(paths) = base["paths"].as_object() else {
        return breaks;
    };
    for (path, old_item) in paths {
        let old_item = resolve(base, old_item);
        let new_item = resolve(new, &new["paths"][path]);
        for method in METHODS {
            let Some(old_operation) = old_item.get(method) else {
                continue;
            };
            let label = format!("{} {path}", method.to_uppercase());
            let Some(new_operation) = new_item.get(method) else {
                breaks.push(format!("operation `{label}` was removed"));
                continue;
            };
            let old_parameters = path_parameters(base, old_item, old_operation);
            for name in path_parameters(new, new_item, new_operation).difference(&old_parameters) {
                breaks.push(format!("`{label}` path parameter `{name}` was added"));
            }
            let old_body = resolve(base, &old_operation["requestBody"]);
            let new_body = resolve(new, &new_operation["requestBody"]);
            if let Some(content) = new_body["content"].as_object() {
                for (media, new_media) in content {
                    // An added alternate media type leaves old representations valid.
                    if old_body["content"].is_object() && old_body["content"].get(media).is_none() {
                        continue;
                    }
                    request_required(
                        (base, new),
                        (&old_body["content"][media]["schema"], &new_media["schema"]),
                        &format!("{label} request {media}"),
                        &mut Vec::new(),
                        &mut breaks,
                    );
                }
            }
        }
    }
    breaks
}

// The request emitter represents Option<T> as T | null. Compare the payload
// branch so required fields inside an existing nullable object remain visible.
fn request_shape<'a>(document: &'a Value, schema: &'a Value) -> &'a Value {
    let schema = resolve(document, schema);
    if let Some(branches) = schema["anyOf"].as_array()
        && branches.len() == 2
        && branches.iter().any(|branch| branch["type"] == "null")
        && let Some(payload) = branches.iter().find(|branch| branch["type"] != "null")
    {
        return resolve(document, payload);
    }
    schema
}

fn request_required<'a>(
    documents: (&'a Value, &'a Value),
    schemas: (&'a Value, &'a Value),
    label: &str,
    ancestors: &mut Vec<(&'a Value, &'a Value)>,
    breaks: &mut Vec<String>,
) {
    let old = request_shape(documents.0, schemas.0);
    let new = request_shape(documents.1, schemas.1);
    if ancestors
        .iter()
        .any(|(a, b)| std::ptr::eq(*a, old) && std::ptr::eq(*b, new))
    {
        return;
    }
    ancestors.push((old, new));
    for name in super::required_set(new).difference(&super::required_set(old)) {
        breaks.push(format!("`{label}` required property `{name}` was added"));
    }
    if let Some(properties) = new["properties"].as_object() {
        for (name, new_property) in properties {
            // A newly optional object is compatible; only compare nested
            // constraints on properties an existing caller could already send.
            if let Some(old_property) = old["properties"].get(name) {
                request_required(
                    documents,
                    (old_property, new_property),
                    &format!("{label}.{name}"),
                    ancestors,
                    breaks,
                );
            }
        }
    }
    if old.get("items").is_some() && new.get("items").is_some() {
        request_required(
            documents,
            (&old["items"], &new["items"]),
            &format!("{label}[]"),
            ancestors,
            breaks,
        );
    }
    ancestors.pop();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn document() -> Value {
        json!({"paths": {"/things/{id}": {
            "parameters": [{"in":"path", "name":"id", "required":true}],
            "get": {},
            "post": {"requestBody":{"$ref":"#/components/requestBodies/Input"}}
        }}, "components": {
            "requestBodies": {"Input":{"content":{"application/json":{"schema":{"$ref":"#/components/schemas/Input"}}}}},
            "schemas": {"Input":{"type":"object", "required":["name"], "properties":{
                "name":{"type":"string"}, "note":{"type":"string"},
                "child":{"$ref":"#/components/schemas/Input"}
            }}}
        }})
    }

    #[test]
    fn removed_operations_and_paths_are_breaks_but_metadata_is_not_an_operation() {
        let base = document();
        let mut new = base.clone();
        new["paths"]["/things/{id}"]
            .as_object_mut()
            .unwrap()
            .remove("get");
        assert_eq!(
            operation_changes(&base, &new),
            ["operation `GET /things/{id}` was removed"]
        );
        new["paths"] = json!({});
        assert_eq!(operation_changes(&base, &new).len(), 2);
    }

    #[test]
    fn new_path_parameters_include_shared_and_referenced_parameters() {
        let base = document();
        let mut new = base.clone();
        new["components"]["parameters"] =
            json!({"extra":{"in":"path","name":"extra","required":true}});
        new["paths"]["/things/{id}"]["post"]["parameters"] =
            json!([{"$ref":"#/components/parameters/extra"}]);
        assert_eq!(
            operation_changes(&base, &new),
            ["`POST /things/{id}` path parameter `extra` was added"]
        );
        // Moving an existing parameter from the path to the operation is neutral.
        new = base.clone();
        let params = new["paths"]["/things/{id}"]
            .as_object_mut()
            .unwrap()
            .remove("parameters")
            .unwrap();
        for method in ["get", "post"] {
            new["paths"]["/things/{id}"][method]["parameters"] = params.clone();
        }
        assert!(operation_changes(&base, &new).is_empty());
    }

    #[test]
    fn newly_required_requests_follow_refs_and_handle_recursive_schemas() {
        let base = document();
        let mut new = base.clone();
        new["components"]["schemas"]["Input"]["required"] = json!(["name", "note", "added"]);
        let breaks = operation_changes(&base, &new);
        assert_eq!(breaks.len(), 2, "{breaks:?}");
        assert!(
            breaks
                .iter()
                .any(|b| b.contains("required property `note`"))
        );
        assert!(
            breaks
                .iter()
                .any(|b| b.contains("required property `added`"))
        );
    }

    #[test]
    fn nested_nullable_array_inputs_are_checked_but_new_media_is_compatible() {
        let mut base = document();
        base["components"]["schemas"]["Input"]["properties"]["children"] = json!({
            "anyOf":[{"type":"array", "items":{"type":"object", "properties":{"id":{"type":"string"}}}}, {"type":"null"}]
        });
        let mut new = base.clone();
        new["components"]["schemas"]["Input"]["properties"]["children"]["anyOf"][0]["items"]["required"] =
            json!(["id"]);
        let breaks = operation_changes(&base, &new);
        assert_eq!(breaks.len(), 1);
        assert!(breaks[0].contains("application/json.children[]` required property `id`"));
        new = base.clone();
        new["components"]["requestBodies"]["Input"]["content"]["application/xml"] =
            json!({"schema":{"required":["new_representation"]}});
        assert!(operation_changes(&base, &new).is_empty());
    }

    #[test]
    fn new_operations_optional_properties_and_relaxed_requests_are_compatible() {
        let base = document();
        let mut new = base.clone();
        new["paths"]["/things/{id}"]["delete"] = json!({});
        new["components"]["schemas"]["Input"]["required"] = json!([]);
        new["components"]["schemas"]["Input"]["properties"]["optional"] =
            json!({"type":"object","required":["inner"]});
        assert!(operation_changes(&base, &new).is_empty());
        assert!(operation_changes(&base, &base).is_empty());
    }
}
