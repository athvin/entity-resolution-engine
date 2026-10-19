//! `er-entity-spec` — the parsed and validated entity YAML model.
//!
//! One `entity.yaml` per table becomes an [`Entity`]; a validated set of them is a
//! [`Catalog`]. This crate is the single source the generator, the analyser and the
//! differ all read, but it is deliberately dependency-light: it knows the closed
//! eight-type vocabulary ([`ColumnType`]) and the `generate` block, and nothing
//! about DDL rendering, DataFusion or Arrow. The conversions into `er-schema`'s
//! `TableSpec` and `ff-analysis`'s `RelSchema` live in the crates that consume them,
//! so neither a database nor a planner ever enters this build graph.

mod catalog;
mod entity;
mod types;

pub use catalog::{Catalog, SpecError};
pub use entity::{Column, Entity, GenerateBlock, Identity, Index, Reference, singular};
pub use types::{ColumnType, Language, ParseTypeError, Store, Wire};

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN: &str = r#"
kind: entity
version: 1
name: golden_record
relation: golden_records
description: "One resolved person: the surviving value per attribute."
key: [entity_id]
identity: legacy
generate:
  stores: [duckdb]
  languages: [rust, typescript]
  wire: [arrow, json]
columns:
  - name: entity_id
    type: varchar
    nullable: false
    description: "Persistent entity id (ULID)."
  - name: given_name
    type: varchar
    description: "Surviving given name."
  - name: survivorship_version
    type: varchar
    nullable: false
    description: "Survivorship policy version that produced this row."
  - name: assembled_at
    type: timestamp
    nullable: false
    volatile: true
    description: "Assembly time of this row."
"#;

    fn parse(yaml: &str) -> Result<Entity, serde_norway::Error> {
        serde_norway::from_str(yaml)
    }

    #[test]
    fn parses_a_golden_entity() {
        let entity = parse(GOLDEN).expect("parses");
        assert_eq!(entity.name, "golden_record");
        assert_eq!(entity.relation, "golden_records");
        assert_eq!(entity.version, 1);
        assert_eq!(entity.key, vec!["entity_id"]);
        assert_eq!(entity.columns.len(), 4);
        // nullable defaults to true; the explicit ones are false.
        assert!(!entity.column("entity_id").unwrap().nullable);
        assert!(entity.column("given_name").unwrap().nullable);
        assert!(entity.column("assembled_at").unwrap().volatile);
        assert_eq!(
            entity.column("entity_id").unwrap().col_type,
            ColumnType::Varchar
        );
        assert_eq!(
            entity.qualified_name(Store::Duckdb),
            "lake.main.golden_records"
        );
    }

    #[test]
    fn a_catalog_validates_and_keys_by_relation() {
        let catalog = Catalog::from_entities(vec![parse(GOLDEN).unwrap()]).expect("valid");
        assert!(catalog.by_name("golden_record").is_some());
        assert!(
            catalog
                .by_qualified_name(Store::Duckdb, "golden_records")
                .is_some()
        );
    }

    #[test]
    fn a_ninth_type_is_a_spec_error() {
        let yaml = GOLDEN.replace(
            "type: varchar\n    nullable: false",
            "type: uuid\n    nullable: false",
        );
        assert!(parse(&yaml).is_err());
    }

    #[test]
    fn a_missing_generate_block_is_a_spec_error() {
        let yaml = r#"
kind: entity
version: 1
name: t
relation: t
description: "x"
key: [a]
columns:
  - name: a
    type: varchar
    nullable: false
    description: "x"
"#;
        assert!(parse(yaml).is_err());
    }

    #[test]
    fn a_key_column_must_exist() {
        let yaml = GOLDEN.replace("key: [entity_id]", "key: [nope]");
        let entity = parse(&yaml).unwrap();
        let err = Catalog::from_entities(vec![entity]).unwrap_err();
        assert!(matches!(err, SpecError::Invalid { .. }));
    }

    /// A minimal lake entity that satisfies the identity conventions (V1–V8).
    const V2_LAKE: &str = r#"
kind: entity
version: 1
name: thing
relation: things
description: "x"
key: [id]
natural_key: [record_key]
generate:
  stores: [duckdb]
  languages: [rust]
columns:
  - name: id
    type: varchar
    nullable: false
    description: "sha256 natural id."
  - name: record_key
    type: varchar
    nullable: false
    description: "The natural identity."
  - name: created_at
    type: timestamp
    nullable: false
    description: "Run-logical creation time."
  - name: updated_at
    type: timestamp
    nullable: false
    description: "Run-logical last-write time."
"#;

    fn expect_invalid(yaml: &str, needle: &str) {
        let entity = parse(yaml).unwrap();
        let err = Catalog::from_entities(vec![entity]).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains(needle),
            "expected a {needle:?} failure, got: {text}"
        );
    }

    #[test]
    fn the_identity_conventions_hold_for_a_v2_entity() {
        let entity = parse(V2_LAKE).expect("parses");
        assert!(!entity.is_legacy());
        assert_eq!(entity.natural_key, vec!["record_key"]);
        Catalog::from_entities(vec![entity]).expect("a conforming entity validates");
    }

    #[test]
    fn legacy_skips_the_conventions_and_the_golden_fixture_uses_it() {
        let entity = parse(GOLDEN).expect("parses");
        assert!(entity.is_legacy());
        Catalog::from_entities(vec![entity]).expect("legacy entities validate as before");
    }

    #[test]
    fn v1_v2_v3_the_id_column_is_the_varchar_primary_key_and_leads() {
        expect_invalid(&V2_LAKE.replace("key: [id]", "key: [record_key]"), "V2");
        expect_invalid(&V2_LAKE.replace("key: [id]", "key: [id, record_key]"), "V2");
        expect_invalid(
            &V2_LAKE.replace(
                "  - name: id\n    type: varchar",
                "  - name: id\n    type: bigint",
            ),
            "V1",
        );
        // id declared, but not first.
        let moved = V2_LAKE
            .replace(
                "columns:\n  - name: id\n    type: varchar\n    nullable: false\n    description: \"sha256 natural id.\"\n",
                "columns:\n",
            )
            .replace(
                "  - name: updated_at",
                "  - name: id\n    type: varchar\n    nullable: false\n    description: \"sha256 natural id.\"\n  - name: updated_at",
            );
        expect_invalid(&moved, "V3");
    }

    #[test]
    fn v4_v6_timestamps_exist_and_lake_ones_are_run_logical() {
        expect_invalid(
            &V2_LAKE.replace("- name: updated_at", "- name: upd_at"),
            "V4",
        );
        expect_invalid(
            &V2_LAKE.replace(
                "  - name: created_at\n    type: timestamp\n    nullable: false\n",
                "  - name: created_at\n    type: timestamp\n    nullable: false\n    volatile: true\n",
            ),
            "V6",
        );
    }

    #[test]
    fn v10_a_natural_key_is_real_not_null_and_never_the_universal_columns() {
        expect_invalid(
            &V2_LAKE.replace("natural_key: [record_key]", "natural_key: [nope]"),
            "natural_key column",
        );
        expect_invalid(
            &V2_LAKE.replace(
                "natural_key: [record_key]",
                "natural_key: [record_key, record_key]",
            ),
            "repeats",
        );
        expect_invalid(
            &V2_LAKE.replace("natural_key: [record_key]", "natural_key: [created_at]"),
            "natural_key may not include",
        );
    }

    #[test]
    fn v9_references_must_resolve_against_the_catalog() {
        let referrer = r#"
kind: entity
version: 1
name: child
relation: children
description: "x"
key: [a]
identity: legacy
generate:
  stores: [postgres]
  languages: [rust]
columns:
  - name: a
    type: varchar
    nullable: false
    description: "x"
  - name: parent_id
    type: varchar
    references: { relation: parents, column: id }
    description: "x"
"#;
        let catalog = Catalog::from_entities(vec![parse(referrer).unwrap()])
            .expect("a subset catalog defers the cross-entity check");
        let err = catalog.validate_references().unwrap_err();
        assert!(
            err.to_string().contains("no Postgres entity declares"),
            "got: {err}"
        );
    }

    #[test]
    fn singular_is_mechanical() {
        assert_eq!(singular("orgs"), "org");
        assert_eq!(singular("entities"), "entity");
        assert_eq!(singular("jobs"), "job");
        assert_eq!(singular("api_keys"), "api_key");
        assert_eq!(singular("config_versions"), "config_version");
        assert_eq!(singular("audit_log"), "audit_log");
        assert_eq!(singular("review_queue"), "review_queue");
        assert_eq!(singular("statuses"), "status");
    }

    #[test]
    fn a_lake_entity_may_not_carry_a_default() {
        let yaml = GOLDEN.replace(
            "    description: \"Surviving given name.\"",
            "    default: \"'x'\"\n    description: \"Surviving given name.\"",
        );
        let entity = parse(&yaml).unwrap();
        let err = Catalog::from_entities(vec![entity]).unwrap_err();
        assert!(matches!(err, SpecError::Invalid { .. }));
    }
}
