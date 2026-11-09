//! The control-lane OpenAPI document. It references the generated public wire structs'
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
        er_entities::candidate_key::CandidateKeyWire,
        er_entities::column_profile::ColumnProfileWire,
        er_entities::entity_link::EntityLinkWire,
        er_entities::ingest_batch::IngestBatchWire,
        er_entities::matching_model::MatchingModelWire,
        er_entities::metric_snapshot::MetricSnapshotWire,
        er_entities::metric_snapshot_source::MetricSnapshotSourceWire,
        er_entities::pair_label::PairLabelWire,
        er_entities::profile_finding::ProfileFindingWire,
        er_entities::profile_run::ProfileRunWire,
        er_entities::record_quality::RecordQualityWire,
        er_entities::s3_ingest_cursor::S3IngestCursorWire,
        er_entities::tag::TagWire,
        er_entities::tagging::TaggingWire,
        er_entities::webhook_delivery::WebhookDeliveryWire,
        er_entities::golden_record::GoldenRecordWire,
        er_entities::golden_lineage::GoldenLineageWire,
        er_entities::acct_golden_record::AcctGoldenRecordWire,
        er_entities::acct_golden_lineage::AcctGoldenLineageWire,
        er_entities::job::JobWire,
        er_entities::org::OrgWire,
        er_entities::dataset::DatasetWire,
        er_entities::api_key::ApiKeyWire,
        er_entities::config_version::ConfigVersionWire,
        er_entities::schedule::ScheduleWire,
        er_entities::webhook::WebhookWire,
        er_entities::staged_steward_action::StagedStewardActionWire,
        er_entities::audit_log::AuditLogWire,
        er_entities::task_instance::TaskInstanceWire,
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
    fn secret_columns_are_absent_from_public_contracts() {
        let doc: serde_json::Value = serde_json::from_str(&openapi_json()).unwrap();
        for (entity, field) in [
            ("Webhook", "secret"),
            ("ApiKey", "key_hash"),
            ("TaskInstance", "token_hash"),
        ] {
            let schema = &doc["components"]["schemas"][entity];
            assert!(schema.is_object(), "missing {entity}");
            assert!(
                schema["properties"].get(field).is_none(),
                "{entity}.{field} leaked into OpenAPI"
            );
            assert!(
                !required_set(schema).contains(field),
                "{entity}.{field} is required"
            );
            let root =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../frontend/src/lib");
            let binding =
                std::fs::read_to_string(root.join(format!("entities/bindings/{entity}.ts")))
                    .unwrap();
            let types = std::fs::read_to_string(root.join("api/schema.d.ts")).unwrap();
            // Scope this check to the entity component. Create responses may
            // legitimately return a freshly minted secret once.
            let component = types
                .split_once(&format!("        {entity}: "))
                .expect("entity component")
                .1
                .strip_prefix('{')
                .expect("component object")
                .split_once("\n        };")
                .expect("component end")
                .0;
            for text in [binding.as_str(), component] {
                assert!(
                    !text.contains(&format!("{field}:")) && !text.contains(&format!("{field}?:")),
                    "{entity}.{field} leaked into TypeScript"
                );
            }
        }
    }

    /// Populate every storage field, including nullable secrets, so omission
    /// cannot pass merely because a test row happened to contain no credential.
    fn populated_row(name: &str) -> serde_json::Value {
        use er_entity_spec::ColumnType;
        let catalog = er_entities::catalog::catalog();
        let entity = catalog.by_name(name).unwrap();
        serde_json::Value::Object(
            entity
                .columns
                .iter()
                .map(|c| {
                    let value = match c.col_type {
                        ColumnType::Varchar => serde_json::json!(format!("sentinel-{}", c.name)),
                        ColumnType::Json | ColumnType::ListVarchar => {
                            serde_json::json!(["sentinel"])
                        }
                        ColumnType::Bigint => serde_json::json!(1),
                        ColumnType::Double => serde_json::json!(1.25),
                        ColumnType::Boolean => serde_json::json!(true),
                        ColumnType::Date => serde_json::json!("2026-01-01"),
                        ColumnType::Timestamp => serde_json::json!("2026-01-01T00:00:00Z"),
                    };
                    (c.name.clone(), value)
                })
                .collect(),
        )
    }

    fn assert_projection<Row, Public>(name: &str, secret: &str)
    where
        Row: serde::de::DeserializeOwned,
        Public: serde::Serialize + for<'a> From<&'a Row>,
    {
        let mut expected = populated_row(name);
        let row: Row = serde_json::from_value(expected.clone()).unwrap();
        assert!(expected.as_object_mut().unwrap().remove(secret).is_some());
        let actual = serde_json::to_value(Public::from(&row)).unwrap();
        assert_eq!(
            actual, expected,
            "wire conversion must retain all public values and omit {secret}"
        );
    }

    #[test]
    fn generated_wire_serialization_omits_secrets_and_preserves_public_values() {
        assert_projection::<er_entities::webhook::Webhook, er_entities::webhook::WebhookWire>(
            "webhook", "secret",
        );
        assert_projection::<er_entities::api_key::ApiKey, er_entities::api_key::ApiKeyWire>(
            "api_key", "key_hash",
        );
        assert_projection::<
            er_entities::task_instance::TaskInstance,
            er_entities::task_instance::TaskInstanceWire,
        >("task_instance", "token_hash");
    }

    #[test]
    fn component_properties_match_public_columns_for_every_json_entity() {
        let doc: serde_json::Value = serde_json::from_str(&openapi_json()).unwrap();
        for entity in er_entities::catalog::catalog()
            .entities()
            .iter()
            .filter(|e| e.has_wire(er_entity_spec::Wire::Json))
        {
            let name = er_entity_gen::casing::to_pascal_case(&entity.name);
            let actual: std::collections::BTreeSet<_> =
                doc["components"]["schemas"][&name]["properties"]
                    .as_object()
                    .unwrap_or_else(|| panic!("missing {name}"))
                    .keys()
                    .map(String::as_str)
                    .collect();
            let expected = entity
                .columns
                .iter()
                .filter(|c| c.visibility == er_entity_spec::Visibility::Public)
                .map(|c| c.name.as_str())
                .collect();
            assert_eq!(actual, expected, "public schema drift for {name}");
        }
    }

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

    /// Entities that declare `wire: [json]` but carry no component schema.
    ///
    /// This is a backlog, not a design. `Wire::Json` means the entity's payloads
    /// cross the JSON wire, so every name here is reachable as JSON with nothing
    /// in the document describing it. The list may **shrink** — register the
    /// `ToSchema` above and delete the line — and must not grow: a new
    /// `wire: [json]` entity fails the test below until someone decides which it
    /// is. Registering one is a public-API decision (it becomes a documented
    /// schema that `er-openapi-check` then tracks for breaking changes), which is
    /// why the gate forces the decision rather than making it.
    ///
    /// Empty since #762: all fifteen entities that founded the list crossed the
    /// wire (every one has a read path in `er-server` serving its rows as JSON),
    /// so all fifteen were registered rather than de-wired.
    const JSON_WIRE_WITHOUT_SCHEMA: &[&str] = &[];

    /// The component schemas `ApiDoc` actually emits, by schema name.
    fn registered_schemas() -> std::collections::BTreeSet<String> {
        ApiDoc::openapi()
            .components
            .as_ref()
            .expect("components present")
            .schemas
            .keys()
            .cloned()
            .collect()
    }

    /// Entity names declaring `wire: [json]`, as the struct names utoipa keys a
    /// component schema by. The casing comes from the generator's own helper, so
    /// this cannot disagree with how the struct was actually named.
    fn json_wire_schema_names() -> std::collections::BTreeSet<String> {
        let catalog = er_entities::catalog::catalog();
        // Guard the derivation itself. A catalog that came back empty would make
        // every assertion below vacuous — which is exactly the failure this test
        // replaces, where a hard-coded count passed while the real gap grew.
        assert!(
            catalog.entities().len() > 40,
            "catalog derivation looks broken: {} entities",
            catalog.entities().len()
        );
        let names: std::collections::BTreeSet<String> = catalog
            .entities()
            .iter()
            .filter(|e| e.generate.wire.contains(&er_entity_spec::Wire::Json))
            .map(|e| er_entity_gen::casing::to_pascal_case(&e.name))
            .collect();
        assert!(
            names.len() > 20,
            "json-wire derivation looks broken: {} entities",
            names.len()
        );
        names
    }

    #[test]
    fn no_schema_is_registered_for_an_entity_that_is_not_on_the_json_wire() {
        let stray: Vec<_> = registered_schemas()
            .difference(&json_wire_schema_names())
            .cloned()
            .collect();
        assert!(
            stray.is_empty(),
            "{} schema(s) are registered in ApiDoc for entities that do not \
             declare `wire: [json]`, so the document describes a shape the \
             entity does not claim to serialize: {}\n\nEither add `json` to the \
             entity's `generate.wire` or drop the registration.",
            stray.len(),
            stray.join(", ")
        );
    }

    #[test]
    fn every_json_wire_entity_has_a_schema_or_a_declared_gap() {
        // The real invariant, as a set difference rather than a count: an entity
        // declaring the JSON wire either has a component schema or is named in
        // the backlog above. A count could not see a new entity arriving
        // unregistered, because the count did not change.
        let expected_gap: std::collections::BTreeSet<String> = JSON_WIRE_WITHOUT_SCHEMA
            .iter()
            .map(|n| er_entity_gen::casing::to_pascal_case(n))
            .collect();
        let actual_gap: std::collections::BTreeSet<String> = json_wire_schema_names()
            .difference(&registered_schemas())
            .cloned()
            .collect();

        let newly_missing: Vec<_> = actual_gap.difference(&expected_gap).cloned().collect();
        assert!(
            newly_missing.is_empty(),
            "{} entit(ies) declare `wire: [json]` with no component schema and \
             are not in JSON_WIRE_WITHOUT_SCHEMA: {}\n\nRegister the `ToSchema` \
             in ApiDoc (then regenerate openapi.json and schema.d.ts), or add \
             the name to that list with a reason.",
            newly_missing.len(),
            newly_missing.join(", ")
        );

        let stale: Vec<_> = expected_gap.difference(&actual_gap).cloned().collect();
        assert!(
            stale.is_empty(),
            "JSON_WIRE_WITHOUT_SCHEMA names {} entit(ies) that no longer need \
             to be there: {}\n\nDelete those lines — the list is meant to \
             shrink, and a stale entry hides the next real gap.",
            stale.len(),
            stale.join(", ")
        );
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
