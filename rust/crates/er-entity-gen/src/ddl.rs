//! The DDL emitter. Lake DDL is rendered by `er_schema::model::create_table_sql`
//! fed from the catalog — the same renderer the registry uses today, so the
//! generated statement is byte-for-byte what `er-schema` would have produced.

use er_entity_spec::Entity;

use crate::convert::{ConvertError, to_lake_table_spec};

/// The `CREATE TABLE` statement for an entity's lake relation.
pub fn emit_lake_ddl(entity: &Entity) -> Result<String, ConvertError> {
    let spec = to_lake_table_spec(entity)?;
    Ok(er_schema::model::create_table_sql(&spec)?)
}
