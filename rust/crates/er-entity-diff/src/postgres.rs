//! Reading the live Postgres schema into [`LiveColumn`] values via
//! `information_schema.columns`, normalising the reported types to the entity
//! vocabulary's spelling so the comparison is apples to apples.

use std::collections::BTreeMap;

use tokio_postgres::Client;

use crate::LiveColumn;

/// Read the `public.*` columns of the given relations, in ordinal order, keyed by
/// bare relation name.
///
/// # Errors
/// Propagates the query error.
pub async fn read_postgres_columns(
    client: &Client,
    relations: &[String],
) -> Result<BTreeMap<String, Vec<LiveColumn>>, tokio_postgres::Error> {
    let rows = client
        .query(
            "SELECT table_name, column_name, data_type, is_nullable \
             FROM information_schema.columns \
             WHERE table_schema = 'public' AND table_name = ANY($1) \
             ORDER BY table_name, ordinal_position",
            &[&relations],
        )
        .await?;

    let mut out: BTreeMap<String, Vec<LiveColumn>> = BTreeMap::new();
    for row in rows {
        let table: String = row.get("table_name");
        let name: String = row.get("column_name");
        let data_type: String = row.get("data_type");
        let nullable = row.get::<_, String>("is_nullable") == "YES";
        out.entry(table).or_default().push(LiveColumn {
            name,
            type_name: normalize_pg_type(&data_type),
            nullable,
        });
    }
    Ok(out)
}

/// Map an `information_schema.data_type` to the entity vocabulary's Postgres
/// spelling (what `ColumnType::postgres_type` returns).
fn normalize_pg_type(data_type: &str) -> String {
    match data_type {
        "timestamp with time zone" => "timestamptz",
        "timestamp without time zone" => "timestamp",
        "character varying" => "text",
        "ARRAY" => "text[]",
        other => other,
    }
    .to_string()
}
