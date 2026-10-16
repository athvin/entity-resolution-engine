//! `golden_records` (S4.6/ER-088): assemble one surviving row per entity by
//! reading each attribute's value from the record `golden_lineage` elected. A
//! Rust port of `dbt/models/marts/golden_records.sql` in full-assembly mode. The
//! six `addr_*` columns all come from the single record that won the `address`
//! decision; `metadata` aggregates each source's client metadata by
//! `source_system -> source_record_id`.

use er_config::Config;

use crate::lineage::{ADDRESS_COLUMNS, SCALAR_ATTRIBUTES};

/// Apply steward golden field overrides (the mass-update feature) as a
/// post-assembly UPDATE over `golden_table` from `overrides_table`: one UPDATE
/// per overridable column (the 5 scalar + 6 address golden columns). For each
/// entity carrying an override of that column, the column is set to the override
/// `value` (NULL = the steward cleared the field); entities without an override
/// keep the survivorship-elected value. With `overrides_table` empty this matches
/// zero rows for every statement — a byte-exact no-op, so the determinism
/// contract (golden fixtures) holds. A correlated subquery (not `UPDATE … FROM`)
/// keeps the target/override `entity_id` references unambiguous.
#[must_use]
pub fn golden_overrides_update_sql(golden_table: &str, overrides_table: &str) -> Vec<String> {
    let base = golden_table.rsplit('.').next().unwrap_or(golden_table);
    SCALAR_ATTRIBUTES
        .iter()
        .chain(ADDRESS_COLUMNS.iter())
        .map(|col| {
            let value_expr = if *col == "birth_date" {
                "CAST(o.value AS DATE)"
            } else {
                "o.value"
            };
            format!(
                "UPDATE {golden_table} SET {col} = (\
                   SELECT {value_expr} FROM {overrides_table} o \
                   WHERE o.entity_id = {base}.entity_id AND o.attribute = '{col}') \
                 WHERE {base}.entity_id IN (\
                   SELECT entity_id FROM {overrides_table} WHERE attribute = '{col}')"
            )
        })
        .collect()
}

/// The `golden_records` SELECT: assemble survived attribute values + metadata per
/// entity, reading winners from `lineage_table` and values from `std_table`.
/// `assembled_at` (volatile) is intentionally omitted.
#[must_use]
pub fn golden_records_sql(
    config: &Config,
    lineage_table: &str,
    std_table: &str,
    membership_table: &str,
    entities_table: &str,
) -> String {
    let version = config.versions.survivorship_version.replace('\'', "''");

    let scalar_select = SCALAR_ATTRIBUTES
        .iter()
        .map(|a| format!("    max(r.{a}) filter (where l.attribute = '{a}') as {a}"))
        .collect::<Vec<_>>()
        .join(",\n");
    let address_select = ADDRESS_COLUMNS
        .iter()
        .map(|c| format!("    max(r.{c}) filter (where l.attribute = 'address') as {c}"))
        .collect::<Vec<_>>()
        .join(",\n");

    format!(
        "with source_metadata as (
    select
        m.entity_id,
        r.source_system,
        to_json(map_from_entries(list(
            struct_pack(key := r.source_record_id, value := r.metadata)
            order by r.source_record_id
        ))) as metadata
    from {membership_table} as m
    join {std_table} as r on r.record_key = m.record_key
    join {entities_table} as e on e.entity_id = m.entity_id and e.status = 'active'
    where coalesce(array_length(json_keys(r.metadata)), 0) > 0
    group by m.entity_id, r.source_system
),
entity_metadata as (
    select
        entity_id,
        to_json(map_from_entries(list(
            struct_pack(key := source_system, value := metadata) order by source_system
        ))) as metadata
    from source_metadata
    group by entity_id
),
survived as (
    select
        l.entity_id,
{scalar_select},
{address_select},
        cast('{version}' as VARCHAR) as survivorship_version
    from {lineage_table} as l
    join {std_table} as r on r.record_key = l.record_key
    join {entities_table} as e on e.entity_id = l.entity_id and e.status = 'active'
    group by l.entity_id
)
select s.*, coalesce(m.metadata, '{{}}'::json) as metadata
from survived as s
left join entity_metadata as m using (entity_id)"
    )
}
