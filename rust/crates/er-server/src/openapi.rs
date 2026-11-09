//! The control-lane OpenAPI document. It references the generated public wire structs'
//! `utoipa::ToSchema` directly, so `openapi.json` — and the frontend's
//! `schema.d.ts`, regenerated from it — follow the entities with no hand-written
//! duplicate. The old document and `scripts/gen-api-types.mjs` were emitted from a
//! since-deleted Python FastAPI app; this is the Rust-owned replacement.
//!
//! The component schemas are the contract this pins; a Rust snapshot test holds
//! `openapi.json` byte-stable and `openapi-typescript` pins `schema.d.ts` from it.
//! Operations are derived from the router, handler signatures and response
//! constructors through `er-api-spec`, with no parallel route annotation table.

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
    let mut doc = ApiDoc::openapi();
    add_operations(&mut doc);
    let mut json = doc
        .to_pretty_json()
        .expect("serialize the OpenAPI document");
    json.push('\n');
    json
}

fn add_operations(doc: &mut utoipa::openapi::OpenApi) {
    use utoipa::openapi::{
        Required,
        path::{
            HttpMethod, OperationBuilder, ParameterBuilder, ParameterIn, PathItem, PathsBuilder,
        },
        request_body::RequestBodyBuilder,
        security::{HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme},
    };
    let components = doc.components.as_mut().expect("registered entity schemas");
    components.add_security_scheme("bearerAuth", SecurityScheme::Http(
        HttpBuilder::new().scheme(HttpAuthScheme::Bearer)
            .description(Some("Tenant API key erk_{key_id}_{secret}, scoped to its organization and role. Operator operations require the separately configured operator bearer token; tenant keys cannot grant operator access. Roles form viewer < steward < admin < operator."))
            .build(),
    ));
    let schemas = components.schemas.keys().cloned().collect();
    let operations = er_api_spec::operations(
        include_str!("lib.rs"),
        &[("mail", include_str!("mail.rs"))],
        &schemas,
    )
    .unwrap_or_else(|e| panic!("derive OpenAPI operations: {e}"));
    let mut paths = PathsBuilder::new();
    for derived in operations {
        let mut operation = OperationBuilder::new()
            .operation_id(Some(&derived.name))
            .summary(Some(&derived.summary))
            .securities(Some(if derived.role == "public" {
                vec![]
            } else {
                // OpenAPI 3.1 permits role names for non-OAuth security schemes.
                vec![SecurityRequirement::new("bearerAuth", [&derived.role])]
            }))
            .responses(operation_responses(&derived.responses));
        for param in derived.params {
            operation = operation.parameter(
                ParameterBuilder::new()
                    .name(param.name)
                    .parameter_in(ParameterIn::Path)
                    .required(Required::True)
                    .schema(Some(json_schema(param.schema)))
                    .build(),
            );
        }
        if let Some(request) = derived.request {
            operation = operation.request_body(Some(
                RequestBodyBuilder::new()
                    .required(Some(if request.required {
                        Required::True
                    } else {
                        Required::False
                    }))
                    .content(request.media_type, json_content(request.schema))
                    .build(),
            ));
        }
        let method = match derived.method.as_str() {
            "GET" => HttpMethod::Get,
            "POST" => HttpMethod::Post,
            "PUT" => HttpMethod::Put,
            "PATCH" => HttpMethod::Patch,
            "DELETE" => HttpMethod::Delete,
            other => panic!("unsupported derived method {other}"),
        };
        paths = paths.path(derived.path, PathItem::new(method, operation.build()));
    }
    doc.paths = paths.build();
}

fn json_schema(mut value: serde_json::Value) -> utoipa::openapi::RefOr<utoipa::openapi::Schema> {
    prepare_utoipa_schema(&mut value);
    serde_json::from_value(value.clone())
        .unwrap_or_else(|e| panic!("derived JSON Schema {value} is unsupported by utoipa: {e}"))
}

/// utoipa deserializes its typeless SchemaType::AnyValue from null, although
/// serialization correctly omits `type`. Fill that representation only at schema
/// nodes; a properties map is not itself a schema. Published JSON never has it.
fn prepare_utoipa_schema(value: &mut serde_json::Value) {
    let Some(schema) = value.as_object_mut() else {
        return;
    };
    if schema.contains_key("$ref") {
        return;
    }
    schema.entry("type").or_insert(serde_json::Value::Null);
    if let Some(properties) = schema
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
    {
        for property in properties.values_mut() {
            prepare_utoipa_schema(property);
        }
    }
    for key in ["items", "additionalProperties"] {
        if let Some(child) = schema.get_mut(key) {
            prepare_utoipa_schema(child);
        }
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(children) = schema
            .get_mut(key)
            .and_then(serde_json::Value::as_array_mut)
        {
            for child in children {
                prepare_utoipa_schema(child);
            }
        }
    }
}

fn json_content(value: serde_json::Value) -> utoipa::openapi::Content {
    utoipa::openapi::Content::new(Some(json_schema(value)))
}

fn operation_responses(
    envelopes: &[er_api_spec::Envelope],
) -> utoipa::openapi::response::Responses {
    use er_api_spec::Envelope;
    use serde_json::json;
    use utoipa::openapi::response::{ResponseBuilder, ResponsesBuilder};
    let mut response = ResponseBuilder::new().description(
        "Successful response. Payload types are described only at envelope granularity.",
    );
    let mut schemas = Vec::new();
    for envelope in envelopes {
        let schema = match envelope {
            // A bare or typed JSON value can be an array, object or scalar. Do
            // not guess a payload type from the variable's name.
            Envelope::Bare | Envelope::Scalar => json!({}),
            Envelope::Object(keys) => json!({"type":"object", "properties": keys.iter()
                .map(|key|(key.clone(), json!({}))).collect::<serde_json::Map<_,_>>()}),
            Envelope::Page => json!({"type":"object", "properties":{
                "items":{"type":"array","items":{}},
                "next_cursor":{"type":["string","null"]}}, "required":["items","next_cursor"]}),
            Envelope::Arrow => {
                response = response.content(
                    "application/vnd.apache.arrow.stream",
                    json_content(json!({"type":"string","format":"binary"})),
                );
                continue;
            }
        };
        if !schemas.contains(&schema) {
            schemas.push(schema);
        }
    }
    if !schemas.is_empty() {
        let schema = if schemas.len() == 1 {
            schemas.remove(0)
        } else {
            json!({"anyOf":schemas})
        };
        response = response.content("application/json", json_content(schema));
    }
    ResponsesBuilder::new().response("2XX", response.build())
        .response("default", ResponseBuilder::new()
            .description("Application error. Request-extractor or infrastructure rejections may use a different body.")
            .content("application/json", json_content(json!({"type":"object", "properties":{"detail":{"type":"string"}}, "required":["detail"]})))
            .build()).build()
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
    fn operations_cover_the_router_and_generate_typescript_paths() {
        let doc: serde_json::Value = serde_json::from_str(&openapi_json()).unwrap();
        let paths = doc["paths"].as_object().unwrap();
        assert!(paths.len() > 80, "operation derivation lost the router");
        for route in er_api_spec::routes(include_str!("lib.rs")) {
            if route.path.starts_with("/v1/internal/") {
                continue;
            }
            for method in er_api_spec::verbs_in(route.handlers) {
                let operation = &paths[route.path][method.to_lowercase()];
                assert!(operation.is_object(), "missing {method} {}", route.path);
                assert!(operation["summary"].as_str().is_some_and(|s| !s.is_empty()));
                for param in er_api_spec::path_params(route.path) {
                    assert!(operation["parameters"].as_array().unwrap().iter().any(|p| {
                        p["name"] == param && p["in"] == "path" && p["required"] == true
                    }));
                }
            }
        }
        let types = include_str!("../../../../frontend/src/lib/api/schema.d.ts");
        assert!(!types.contains("export type paths = Record<string, never>"));
        assert!(types.contains("\"/v1/orgs/{org}/datasets/{ds}/jobs\""));
    }

    #[test]
    fn bearer_security_describes_roles_and_public_health_stays_public() {
        let doc: serde_json::Value = serde_json::from_str(&openapi_json()).unwrap();
        let scheme = &doc["components"]["securitySchemes"]["bearerAuth"];
        assert_eq!(scheme["type"], "http");
        assert_eq!(scheme["scheme"], "bearer");
        assert!(scheme["description"].as_str().unwrap().contains("erk_"));
        for tool in er_api_spec::derive_tools(include_str!("lib.rs")) {
            let path = crate::rewrite_action_path(&tool.path).unwrap_or_else(|| tool.path.clone());
            let operation = &doc["paths"][&path][tool.method.to_lowercase()];
            if tool.role == "public" {
                assert_eq!(operation["security"], serde_json::json!([]));
            } else {
                assert_eq!(
                    operation["security"],
                    serde_json::json!([{"bearerAuth": [tool.role]}])
                );
            }
        }
    }

    #[test]
    fn get_responses_agree_with_every_committed_envelope_class() {
        let doc: serde_json::Value = serde_json::from_str(&openapi_json()).unwrap();
        let ledger: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../frontend/src/lib/api/envelopes.json"
        ))
        .unwrap();
        let mut classes = std::collections::BTreeSet::new();
        for (route, envelope) in ledger.as_object().unwrap() {
            let path = route.strip_prefix("GET ").unwrap();
            if path.starts_with("/v1/internal/") {
                continue;
            }
            let content = &doc["paths"][path]["get"]["responses"]["2XX"]["content"];
            let schema = &content["application/json"]["schema"];
            let class = envelope["envelope"].as_str().unwrap();
            classes.insert(class);
            match class {
                "bare" | "scalar" => assert_eq!(schema, &serde_json::json!({}), "{route}"),
                "wrapped" => assert!(
                    schema["properties"]
                        .get(envelope["key"].as_str().unwrap())
                        .is_some(),
                    "{route}"
                ),
                "record" => {
                    for key in envelope["keys"].as_array().unwrap() {
                        assert!(
                            schema["properties"].get(key.as_str().unwrap()).is_some(),
                            "{route}"
                        );
                    }
                }
                "page" => {
                    assert_eq!(schema["properties"]["items"]["type"], "array");
                    assert_eq!(
                        schema["properties"]["next_cursor"]["type"],
                        serde_json::json!(["string", "null"])
                    );
                }
                "arrow" => {
                    assert!(content.get("application/json").is_none());
                    assert_eq!(
                        content["application/vnd.apache.arrow.stream"]["schema"]["format"],
                        "binary"
                    );
                }
                other => panic!("unsupported ledger class {other}"),
            }
        }
        assert_eq!(
            classes,
            std::collections::BTreeSet::from([
                "bare", "wrapped", "record", "page", "scalar", "arrow"
            ])
        );
    }

    #[test]
    fn upload_optional_body_and_delegated_write_are_described() {
        let doc: serde_json::Value = serde_json::from_str(&openapi_json()).unwrap();
        let paths = &doc["paths"];
        let upload = &paths["/v1/orgs/{org}/datasets/{ds}/imports"]["post"];
        assert!(
            upload["requestBody"]["content"]
                .get("application/json")
                .is_none()
        );
        assert_eq!(
            upload["requestBody"]["content"]["multipart/form-data"]["schema"]["required"],
            serde_json::json!(["file"])
        );
        assert!(
            upload["responses"]["2XX"]["content"]["application/json"]["schema"]["properties"]
                .get("delivery_id")
                .is_some()
        );
        let clear =
            &paths["/v1/orgs/{org}/datasets/{ds}/jobs/{job_id}/tasks/{task_id}/clear"]["post"];
        assert_eq!(clear["requestBody"]["required"], false);
        assert_eq!(
            clear["responses"]["default"]["content"]["application/json"]["schema"]["properties"]["detail"]
                ["type"],
            "string"
        );
        let suspend = &paths["/v1/orgs/{org}/suspend"]["post"];
        assert!(suspend["requestBody"].is_null());
        assert!(
            suspend["responses"]["2XX"]["content"]["application/json"]["schema"]["properties"]
                .get("state")
                .is_some()
        );
    }

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
