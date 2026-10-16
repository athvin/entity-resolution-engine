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
pub use entity::{Column, Entity, GenerateBlock, Index, Reference};
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
