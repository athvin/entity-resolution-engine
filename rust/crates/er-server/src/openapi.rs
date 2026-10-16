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
}
