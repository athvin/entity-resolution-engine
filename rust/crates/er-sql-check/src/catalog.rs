//! Building the analyser's catalog: one `ff_analysis::RelSchema` per entity per
//! store, keyed by the store-qualified relation name (`lake.main.golden_records`,
//! `public.jobs`) so both stores coexist in one `SchemaCatalog`. These are
//! feather-flow's own types, unchanged, so the planner consumes them untranslated.

use std::sync::Arc;

use er_entity_spec::{Catalog, ColumnType, Store};
use ff_analysis::{FloatBitWidth, IntBitWidth};
use ff_analysis::{Nullability, RelSchema, SchemaCatalog, SqlType, TypedColumn};

/// Map an entity column type onto the feather-flow `SqlType` subset. The eight
/// entity types land on a fixed subset; the rest of `SqlType` (decimal, uuid,
/// struct, map) stays available only to intermediate expressions in a statement.
fn to_sql_type(col_type: ColumnType) -> SqlType {
    match col_type {
        ColumnType::Varchar => SqlType::String { max_length: None },
        ColumnType::Bigint => SqlType::Integer {
            bits: IntBitWidth::I64,
        },
        ColumnType::Double => SqlType::Float {
            bits: FloatBitWidth::F64,
        },
        ColumnType::Boolean => SqlType::Boolean,
        ColumnType::Date => SqlType::Date,
        ColumnType::Timestamp => SqlType::Timestamp,
        ColumnType::Json => SqlType::Json,
        ColumnType::ListVarchar => SqlType::Array(Box::new(SqlType::String { max_length: None })),
    }
}

/// Build a `RelSchema` for one entity in a given store: its columns in
/// declaration order, each carrying the entity's own relation as `source_table`.
fn rel_schema(entity: &er_entity_spec::Entity) -> RelSchema {
    let columns = entity
        .columns
        .iter()
        .map(|c| TypedColumn {
            name: c.name.clone(),
            source_table: Some(entity.relation.clone()),
            sql_type: to_sql_type(c.col_type),
            nullability: if c.nullable {
                Nullability::Nullable
            } else {
                Nullability::NotNull
            },
            provenance: vec![],
        })
        .collect();
    RelSchema::new(columns)
}

/// Build the analyser catalog for one store from the entity catalog. Only entities
/// generated for that store are included, keyed by their store-qualified relation.
#[must_use]
pub fn build_store_catalog(catalog: &Catalog, store: Store) -> SchemaCatalog {
    let mut out = SchemaCatalog::new();
    for entity in catalog.for_store(store) {
        out.insert(entity.qualified_name(store), Arc::new(rel_schema(entity)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN_YAML: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../entities/golden_record/entity.yaml"
    ));

    fn golden_catalog() -> Catalog {
        let entity = serde_norway::from_str(GOLDEN_YAML).expect("parses");
        Catalog::from_entities(vec![entity]).expect("valid")
    }

    #[test]
    fn lake_catalog_is_keyed_by_qualified_name() {
        let cat = build_store_catalog(&golden_catalog(), Store::Duckdb);
        let rel = cat
            .get("lake.main.golden_records")
            .expect("keyed by store-qualified name");
        assert_eq!(rel.columns.len(), 17);
        let entity_id = rel.find_column("id").expect("has id");
        assert_eq!(entity_id.nullability, Nullability::NotNull);
        assert!(matches!(
            entity_id.sql_type,
            SqlType::String { max_length: None }
        ));
    }

    #[test]
    fn a_postgres_only_store_is_empty_for_a_lake_entity() {
        let cat = build_store_catalog(&golden_catalog(), Store::Postgres);
        assert!(cat.is_empty());
    }
}
