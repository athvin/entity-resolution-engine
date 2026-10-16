//! Converting a spec [`Entity`] into the `er-schema` catalog types the DDL renderer
//! consumes. This is the one place the spec vocabulary meets `er-schema`'s; the
//! lake DDL is then rendered by `er_schema::model::create_table_sql` unchanged.

use std::collections::{BTreeMap, BTreeSet};

use er_entity_spec::{Entity, Store};
use er_schema::model::{Column, LogicalKey, Owner, SchemaError, TableSpec};

/// A conversion failure (an invalid type reaching `er-schema`, which should never
/// happen for a validated catalog but is surfaced rather than panicked).
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    /// The entity is not generated for the requested store.
    #[error("{entity} is not generated for the {store:?} store")]
    WrongStore { entity: String, store: Store },
    /// `er-schema` rejected a column or spec.
    #[error(transparent)]
    Schema(#[from] SchemaError),
}

/// Build the `er-schema` [`TableSpec`] for an entity's lake relation. The spec is
/// `Owner::Ddl` — the entity framework is the single owner of every lake table's
/// DDL; there is no dbt second owner.
pub fn to_lake_table_spec(entity: &Entity) -> Result<TableSpec, ConvertError> {
    if !entity.has_store(Store::Duckdb) {
        return Err(ConvertError::WrongStore {
            entity: entity.name.clone(),
            store: Store::Duckdb,
        });
    }

    let columns = entity
        .columns
        .iter()
        .map(|c| Column::new(c.name.clone(), c.col_type.ducklake_type(), c.nullable))
        .collect::<Result<Vec<_>, _>>()?;

    let keys = vec![LogicalKey {
        columns: entity.key.clone(),
        r#where: None,
    }];

    // accepted_values become `∈ {…}` domains (never present on a lake entity today,
    // but the mapping is here so a dual-store entity renders the same way).
    let mut enums: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for column in &entity.columns {
        if let Some(values) = &column.accepted_values {
            enums.insert(column.name.clone(), values.iter().cloned().collect());
        }
    }

    Ok(TableSpec::new(
        entity.relation.clone(),
        Owner::Ddl,
        columns,
        keys,
        enums,
    )?)
}
