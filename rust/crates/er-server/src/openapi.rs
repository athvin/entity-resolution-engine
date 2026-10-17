//! The control-lane OpenAPI document. It references the generated entity structs'
//! `utoipa::ToSchema` directly, so `openapi.json` — and the frontend's
//! `schema.d.ts`, regenerated from it — follow the entities with no hand-written
//! duplicate. The old document and `scripts/gen-api-types.mjs` were emitted from a
//! since-deleted Python FastAPI app; this is the Rust-owned replacement.
//!
//! The component schemas are the contract this pins; a Rust snapshot test holds
//! `openapi.json` byte-stable and `openapi-typescript` pins `schema.d.ts` from it.
//! (Per-path operation docs ride on top later; the type contract is what the
//! split backend/frontend most needed.)

use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "er-server",
        version = "1.0.0",
        description = "Entity Resolution control + read API. Component schemas are generated from the entity definitions (entities/*/entity.yaml) via er-entity-gen; this document is their single serialized form.",
    ),
    components(schemas(
        er_entities::golden_record::GoldenRecord,
        er_entities::golden_lineage::GoldenLineage,
        er_entities::golden_display::GoldenDisplay,
        er_entities::acct_golden_record::AcctGoldenRecord,
        er_entities::acct_golden_lineage::AcctGoldenLineage,
        er_entities::acct_golden_display::AcctGoldenDisplay,
        er_entities::job::Job,
        er_entities::org::Org,
        er_entities::api_key::ApiKey,
        er_entities::config_version::ConfigVersion,
        er_entities::schedule::Schedule,
        er_entities::webhook::Webhook,
        er_entities::staged_steward_action::StagedStewardAction,
        er_entities::audit_log::AuditLog,
    ))
)]
pub struct ApiDoc;

/// The OpenAPI document as pretty-printed JSON — the exact bytes committed to
/// `frontend/src/lib/api/openapi.json` and pinned by the snapshot test.
#[must_use]
pub fn openapi_json() -> String {
    let mut json = ApiDoc::openapi()
        .to_pretty_json()
        .expect("serialize the OpenAPI document");
    json.push('\n');
    json
}

/// The component-schema map of an OpenAPI document (`components.schemas`), empty
/// if the document is malformed or carries none.
fn component_schemas(doc: &serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    doc.get("components")
        .and_then(|c| c.get("schemas"))
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The set of a schema's required field names.
fn required_set(schema: &serde_json::Value) -> std::collections::BTreeSet<String> {
    schema
        .get("required")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The breaking changes between two OpenAPI documents' component schemas: a schema
/// removed, a property removed, a property whose `type` changed, or a property that
/// is no longer required (each breaks a consumer generated against the old shape).
/// Additions — a new schema, a new property — are backward-compatible and not
/// reported. The rule set is deliberately small; the gate is advisory until the
/// first external org key, blocking after.
#[must_use]
pub fn breaking_changes(base_json: &str, new_json: &str) -> Vec<String> {
    let base: serde_json::Value =
        serde_json::from_str(base_json).unwrap_or(serde_json::Value::Null);
    let new: serde_json::Value = serde_json::from_str(new_json).unwrap_or(serde_json::Value::Null);
    let base_schemas = component_schemas(&base);
    let new_schemas = component_schemas(&new);

    let mut breaks = Vec::new();
    for (name, base_schema) in &base_schemas {
        let Some(new_schema) = new_schemas.get(name) else {
            breaks.push(format!("schema `{name}` was removed"));
            continue;
        };
        // A required field dropped from `required` relaxes the contract a consumer
        // relied on (the field may now be absent).
        let base_required = required_set(base_schema);
        let new_required = required_set(new_schema);
        for field in base_required.difference(&new_required) {
            breaks.push(format!("`{name}.{field}` is no longer required"));
        }
        // A removed property, or a property whose declared `type` changed.
        let base_props = base_schema
            .get("properties")
            .and_then(serde_json::Value::as_object);
        let new_props = new_schema
            .get("properties")
            .and_then(serde_json::Value::as_object);
        if let Some(base_props) = base_props {
            for (field, base_prop) in base_props {
                match new_props.and_then(|p| p.get(field)) {
                    None => breaks.push(format!("`{name}.{field}` was removed")),
                    Some(new_prop) => {
                        if base_prop.get("type") != new_prop.get("type") {
                            breaks.push(format!(
                                "`{name}.{field}` type changed from {} to {}",
                                base_prop.get("type").unwrap_or(&serde_json::Value::Null),
                                new_prop.get("type").unwrap_or(&serde_json::Value::Null)
                            ));
                        }
                    }
                }
            }
        }
    }
    breaks.sort();
    breaks
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMITTED: &str = include_str!("../../../../frontend/src/lib/api/openapi.json");

    #[test]
    fn openapi_json_matches_the_committed_document() {
        assert_eq!(
            openapi_json(),
            COMMITTED,
            "the committed frontend/src/lib/api/openapi.json is stale; run \
             `cargo run -p er-server --bin er-openapi > frontend/src/lib/api/openapi.json` \
             and regenerate schema.d.ts (pnpm gen:api), then commit"
        );
    }

    #[test]
    fn every_generated_entity_schema_is_registered() {
        // The document carries one component schema per generated entity (14).
        let doc = ApiDoc::openapi();
        let schemas = doc
            .components
            .as_ref()
            .expect("components present")
            .schemas
            .len();
        assert_eq!(schemas, 14, "expected one schema per generated entity");
    }

    fn doc(schemas: &str) -> String {
        format!(r#"{{"openapi":"3.1.0","components":{{"schemas":{schemas}}}}}"#)
    }

    #[test]
    fn additive_changes_are_not_breaking() {
        let base = doc(
            r#"{"Job":{"type":"object","required":["id"],"properties":{"id":{"type":"string"}}}}"#,
        );
        // A new schema + a new optional property on Job: both backward-compatible.
        let new = doc(
            r#"{"Job":{"type":"object","required":["id"],"properties":{"id":{"type":"string"},"note":{"type":"string"}}},"Org":{"type":"object"}}"#,
        );
        assert_eq!(breaking_changes(&base, &new), Vec::<String>::new());
    }

    #[test]
    fn removed_schema_removed_property_type_change_and_dropped_required_are_breaking() {
        let base = doc(
            r#"{"Job":{"type":"object","required":["id","kind"],"properties":{"id":{"type":"string"},"kind":{"type":"string"},"priority":{"type":"integer"}}},"Org":{"type":"object"}}"#,
        );
        // Org removed; Job.priority removed; Job.id retyped; Job.kind no longer required.
        let new = doc(
            r#"{"Job":{"type":"object","required":["id"],"properties":{"id":{"type":"integer"},"kind":{"type":"string"}}}}"#,
        );
        let breaks = breaking_changes(&base, &new);
        assert!(
            breaks
                .iter()
                .any(|b| b.contains("schema `Org` was removed")),
            "{breaks:?}"
        );
        assert!(
            breaks
                .iter()
                .any(|b| b.contains("`Job.priority` was removed")),
            "{breaks:?}"
        );
        assert!(
            breaks.iter().any(|b| b.contains("`Job.id` type changed")),
            "{breaks:?}"
        );
        assert!(
            breaks
                .iter()
                .any(|b| b.contains("`Job.kind` is no longer required")),
            "{breaks:?}"
        );
    }

    #[test]
    fn the_document_is_compatible_with_itself() {
        let json = openapi_json();
        assert_eq!(breaking_changes(&json, &json), Vec::<String>::new());
    }
}
