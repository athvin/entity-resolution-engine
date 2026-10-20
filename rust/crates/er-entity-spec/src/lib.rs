//! `er-entity-spec` — the parsed entity YAML model.
//!
//! One `entity.yaml` per table becomes an [`Entity`]; a structurally coherent set
//! of them is a [`Catalog`]. This crate is the single source the generator, the
//! analyser and the differ all read, but it is deliberately dependency-light: it
//! knows the closed eight-type vocabulary ([`ColumnType`]) and the `generate`
//! block, and nothing about DDL rendering, DataFusion or Arrow. The conversions
//! into `er-schema`'s `TableSpec` and `ff-analysis`'s `RelSchema` live in the
//! crates that consume them, so neither a database nor a planner ever enters this
//! build graph.
//!
//! **Loading checks structure, not convention.** [`Catalog::load_dir`] rejects
//! only what would make a catalog incoherent for its own consumers — a duplicate
//! column name, a key naming no column, `serial` on a non-bigint. The conventions
//! every entity must hold (the V-rules: `id` is the key, the universal
//! timestamps, foreign-key naming, what each store will honour) live in
//! `er-entity-lint`, which reports every violation at once and carries each
//! rule's code and reasoning as data. Load through
//! `er_entity_lint::load_linted` anywhere a generated artifact is about to be
//! written.

mod catalog;
mod entity;
mod types;

pub use catalog::{Catalog, SpecError};
pub use entity::{Column, Entity, GenerateBlock, Index, Reference, singular};
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
key: [id]
generate:
  stores: [duckdb]
  languages: [rust, typescript]
  wire: [arrow, json]
columns:
  - name: id
    type: varchar
    nullable: false
    description: "Persistent entity id (deterministic UUIDv7)."
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
  - name: created_at
    type: timestamp
    nullable: false
    description: "Run-logical creation time."
  - name: updated_at
    type: timestamp
    nullable: false
    description: "Run-logical last-write time."
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
        assert_eq!(entity.key, vec!["id"]);
        assert_eq!(entity.columns.len(), 6);
        // nullable defaults to true; the explicit ones are false.
        assert!(!entity.column("id").unwrap().nullable);
        assert!(entity.column("given_name").unwrap().nullable);
        assert!(entity.column("assembled_at").unwrap().volatile);
        assert_eq!(entity.column("id").unwrap().col_type, ColumnType::Varchar);
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
        let yaml = GOLDEN.replace("key: [id]", "key: [nope, id]");
        let entity = parse(&yaml).unwrap();
        let err = Catalog::from_entities(vec![entity]).unwrap_err();
        assert!(matches!(err, SpecError::Invalid { .. }));
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
}
