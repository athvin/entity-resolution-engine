//! `golden_records` (S4.6/ER-088): assemble one surviving row per entity by
//! reading each attribute's value from the record `golden_lineage` elected. A
//! Rust port of the retired dbt model `golden_records.sql` in full-assembly mode. The
//! six `addr_*` columns all come from the single record that won the `address`
//! decision; `metadata` aggregates each source's client metadata by
//! `source_system -> source_record_id`.

use er_config::Config;

use crate::lineage::{
    ACCT_ADDRESS_COLUMNS, ACCT_SCALAR_ATTRIBUTES, ADDRESS_COLUMNS, SCALAR_ATTRIBUTES,
};
use crate::survivorship::synthesized_value_expr;

/// The per-scalar-attribute projection for the `survived` CTE. A Bucket A / base
/// chain reads the single winning record's column (`max(... ) filter`, elected by
/// `golden_lineage`); a Bucket B chain (PR 1.6b `combined_list`/`sum`) instead
/// SYNTHESIZES the value across every active member via a correlated aggregate —
/// a synthesized value has no owning record, so it is not re-read from lineage's
/// `record_key`. With no Bucket B chain configured (e.g. base_10) every arm is the
/// unchanged `max(... ) filter`, so the golden output is byte-identical.
fn scalar_projection(
    config: &Config,
    attributes: &[&str],
    std_table: &str,
    membership_table: &str,
    entities_table: &str,
) -> String {
    attributes
        .iter()
        .map(|a| {
            let synth = config
                .survivorship
                .get(*a)
                .and_then(|chain| synthesized_value_expr(chain, &format!("rr.{a}")));
            match synth {
                Some(agg) => format!(
                    "    (select {agg} from {membership_table} mm \
                       join {std_table} rr on rr.record_key = mm.record_key \
                       join {entities_table} ee on ee.id = mm.entity_id \
                        and ee.status = 'active' \
                     where mm.entity_id = l.entity_id) as {a}"
                ),
                None => format!("    max(r.{a}) filter (where l.attribute = '{a}') as {a}"),
            }
        })
        .collect::<Vec<_>>()
        .join(",\n")
}

/// The `golden_records` SELECT: assemble survived attribute values + metadata per
/// entity, reading winners from `lineage_table` and values from `std_table`.
/// `assembled_at` is the UTC assembly time (`volatile`: excluded from determinism
/// comparisons, but a materialised NOT NULL column the read API streams).
#[must_use]
pub fn golden_records_sql(
    config: &Config,
    lineage_table: &str,
    std_table: &str,
    membership_table: &str,
    entities_table: &str,
    run_at: &str,
) -> String {
    let version = config.versions.survivorship_version.replace('\'', "''");
    let run_at = run_at.replace('\'', "''");

    let scalar_select = scalar_projection(
        config,
        &SCALAR_ATTRIBUTES,
        std_table,
        membership_table,
        entities_table,
    );
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
    join {entities_table} as e on e.id = m.entity_id and e.status = 'active'
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
        l.entity_id as id,
{scalar_select},
{address_select},
        cast('{version}' as VARCHAR) as survivorship_version
    from {lineage_table} as l
    join {std_table} as r on r.record_key = l.record_key
    join {entities_table} as e on e.id = l.entity_id and e.status = 'active'
    group by l.entity_id
)
select s.*, coalesce(m.metadata, '{{}}'::json) as metadata,
       (now() at time zone 'UTC') as assembled_at,
       cast('{run_at}' as timestamp) as created_at,
       cast('{run_at}' as timestamp) as updated_at
from survived as s
left join entity_metadata as m on m.entity_id = s.id"
    )
}

/// The Account `acct_golden_records` assembly — parallel to [`golden_records_sql`]
/// over the Account survivable columns (`company_name, domain, phone_e164` +
/// the four `addr_*` under the `address` token), including the same volatile
/// `assembled_at`. The person builder's determinism fixtures compare explicit
/// columns (never `assembled_at`), so they do not move.
#[must_use]
pub fn acct_golden_records_sql(
    config: &Config,
    lineage_table: &str,
    std_table: &str,
    membership_table: &str,
    entities_table: &str,
    run_at: &str,
) -> String {
    let version = config.versions.survivorship_version.replace('\'', "''");
    let run_at = run_at.replace('\'', "''");

    let scalar_select = scalar_projection(
        config,
        &ACCT_SCALAR_ATTRIBUTES,
        std_table,
        membership_table,
        entities_table,
    );
    let address_select = ACCT_ADDRESS_COLUMNS
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
    join {entities_table} as e on e.id = m.entity_id and e.status = 'active'
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
        l.entity_id as id,
{scalar_select},
{address_select},
        cast('{version}' as VARCHAR) as survivorship_version
    from {lineage_table} as l
    join {std_table} as r on r.record_key = l.record_key
    join {entities_table} as e on e.id = l.entity_id and e.status = 'active'
    group by l.entity_id
)
select s.*, coalesce(m.metadata, '{{}}'::json) as metadata,
       (now() at time zone 'UTC') as assembled_at,
       cast('{run_at}' as timestamp) as created_at,
       cast('{run_at}' as timestamp) as updated_at
from survived as s
left join entity_metadata as m on m.entity_id = s.id"
    )
}
