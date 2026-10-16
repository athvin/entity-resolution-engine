//! Reading the live DuckLake schema into [`LiveColumn`] values. DuckLake exposes
//! the attached catalog through DuckDB's `information_schema`; the reported types
//! are normalised to the entity vocabulary's spelling (`VARCHAR[]` ->
//! `LIST(VARCHAR)`).

use std::collections::BTreeMap;

use crate::LiveColumn;

/// Read the `lake.main.*` columns, in ordinal order, keyed by bare relation name,
/// restricted to `relations`.
///
/// # Errors
/// Propagates the DuckDB query error.
pub fn read_lake_columns(
    conn: &duckdb::Connection,
    relations: &[String],
) -> duckdb::Result<BTreeMap<String, Vec<LiveColumn>>> {
    let wanted: std::collections::BTreeSet<&str> = relations.iter().map(String::as_str).collect();
    let mut stmt = conn.prepare(
        "SELECT table_name, column_name, data_type, is_nullable \
         FROM information_schema.columns \
         WHERE table_catalog = 'lake' AND table_schema = 'main' \
         ORDER BY table_name, ordinal_position",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;

    let mut out: BTreeMap<String, Vec<LiveColumn>> = BTreeMap::new();
    for row in rows {
        let (table, name, data_type, is_nullable) = row?;
        if !wanted.contains(table.as_str()) {
            continue;
        }
        out.entry(table).or_default().push(LiveColumn {
            name,
            type_name: normalize_lake_type(&data_type),
            nullable: is_nullable == "YES",
        });
    }
    Ok(out)
}

/// Map a DuckDB `information_schema.data_type` to the entity vocabulary's DuckLake
/// spelling (what `ColumnType::ducklake_type` returns).
fn normalize_lake_type(data_type: &str) -> String {
    match data_type {
        "VARCHAR[]" => "LIST(VARCHAR)".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_store;
    use er_entity_spec::{Catalog, Entity, Store};

    const GOLDEN_YAML: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../entities/golden_record/entity.yaml"
    ));

    #[test]
    fn lake_schema_built_from_generated_ddl_has_no_diff() {
        let entity: Entity = serde_norway::from_str(GOLDEN_YAML).unwrap();
        let catalog = Catalog::from_entities(vec![entity.clone()]).unwrap();

        let conn = duckdb::Connection::open_in_memory().unwrap();
        // JSON is DuckDB's bundled type (the metadata column); attach a plain
        // in-memory catalog named `lake` with its default `main` schema.
        let _ = conn.execute_batch("INSTALL json; LOAD json;");
        conn.execute_batch("ATTACH ':memory:' AS lake;").unwrap();
        let ddl = er_entity_gen::emit_lake_ddl(&entity).unwrap();
        conn.execute_batch(&ddl).unwrap();

        let live = read_lake_columns(&conn, &["golden_records".to_string()]).unwrap();
        assert_eq!(live.get("golden_records").map(Vec::len), Some(15));
        assert_eq!(diff_store(&catalog, Store::Duckdb, &live), Vec::new());
    }
}
