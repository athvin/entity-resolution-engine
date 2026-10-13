//! `golden_lineage` (S4.6): one row per `(entity_id, attribute)` naming WHICH
//! member record supplied the golden value and WHICH rule decided it. A Rust
//! port of `dbt/models/marts/golden_lineage.sql` in full-assembly mode
//! (`assemble_entity_filter` = TRUE). The six-token vocabulary
//! `{given_name, family_name, email, phone_e164, birth_date, address}` is the
//! grid; `address` collapses the six `addr_*` columns into one decision.

use anyhow::Result;
use er_config::Config;

use crate::survivorship::survivorship_decision_sql;

/// S4.6 scalar attributes (each its own golden_lineage row).
pub const SCALAR_ATTRIBUTES: [&str; 5] = [
    "given_name",
    "family_name",
    "email",
    "phone_e164",
    "birth_date",
];

/// The six `addr_*` columns collapsed under the `address` token (one decision).
pub const ADDRESS_COLUMNS: [&str; 6] = [
    "addr_number",
    "addr_street",
    "addr_unit",
    "addr_city",
    "addr_region",
    "addr_postal",
];

/// The `<attr>_valid` input column the `validated` rule reads. `int_std_records`
/// spells phone's flag `phone_valid`, so the attr relation aliases it to
/// `phone_e164_valid` (S4.6 names the fragment `<attr>_valid`).
fn validated_column(attribute: &str) -> Option<&'static str> {
    match attribute {
        "email" => Some("email_valid"),
        "phone_e164" => Some("phone_valid"),
        _ => None,
    }
}

/// `MAP { 'src': struct_pack(priority_rank := N), ... }` from the config's source
/// table — the `sources[source_system].priority_rank` the `source_priority` rule
/// looks up. Order is irrelevant to the lookup.
fn sources_map(config: &Config) -> String {
    let entries: Vec<String> = config
        .sources
        .iter()
        .map(|(name, spec)| {
            format!(
                "'{}': struct_pack(priority_rank := {})",
                name.replace('\'', "''"),
                spec.priority_rank
            )
        })
        .collect();
    format!("MAP {{ {} }}", entries.join(", "))
}

/// The `golden_lineage` SELECT over `std_table` (int_std_records),
/// `membership_table` (entity_membership) and `entities_table` (entities). Reads
/// the per-attribute survivorship chains from `config`.
pub fn golden_lineage_sql(
    config: &Config,
    std_table: &str,
    membership_table: &str,
    entities_table: &str,
) -> Result<String> {
    let version = config.versions.survivorship_version.replace('\'', "''");

    let address_composite = ADDRESS_COLUMNS
        .iter()
        .map(|c| format!("coalesce({c}, '')"))
        .collect::<Vec<_>>()
        .join(",\n            ");

    // Per-attribute relations in LONG form: value + the keys every rule reads.
    let mut attr_ctes: Vec<String> = Vec::new();
    for attribute in SCALAR_ATTRIBUTES {
        let valid = validated_column(attribute).map_or(String::new(), |col| {
            format!(",\n        {col} as {attribute}_valid")
        });
        attr_ctes.push(format!(
            "attr_{attribute} as (
    select
        entity_id, record_key, source_system, source_record_id,
        updated_at_source, ingested_at, _sources as sources,
        {attribute} as value{valid}
    from member_rows
)"
        ));
    }
    attr_ctes.push(
        "attr_address as (
    select
        entity_id, record_key, source_system, source_record_id,
        updated_at_source, ingested_at, _sources as sources,
        _address_composite as value
    from member_rows
)"
        .to_string(),
    );

    // Each attribute's winner via its survivorship chain.
    let mut decision_ctes: Vec<String> = Vec::new();
    for attribute in SCALAR_ATTRIBUTES.iter().chain(std::iter::once(&"address")) {
        let chain = config.survivorship.get(*attribute).ok_or_else(|| {
            anyhow::anyhow!("survivorship chain missing for attribute {attribute:?}")
        })?;
        let decision = survivorship_decision_sql(attribute, chain, &format!("attr_{attribute}"))?;
        decision_ctes.push(format!("decision_{attribute} as (\n{decision}\n)"));
    }

    // The grid: one arm per vocabulary token, a literal attribute per arm.
    let grid_arms: Vec<String> = SCALAR_ATTRIBUTES
        .iter()
        .chain(std::iter::once(&"address"))
        .map(|attribute| {
            format!(
                "    select entity_id, '{attribute}' as attribute, record_key, rule\n    \
                 from decision_{attribute}"
            )
        })
        .collect();

    let sources = sources_map(config);
    let attr_block = attr_ctes.join(",\n");
    let decision_block = decision_ctes.join(",\n");
    let grid_block = grid_arms.join("\n    union all\n");

    Ok(format!(
        "with members as (
    select m.entity_id, r.*
    from {std_table} as r
    join {membership_table} as m on m.record_key = r.record_key
    join {entities_table} as e on e.entity_id = m.entity_id and e.status = 'active'
),
member_rows as (
    select
        *,
        {sources} as _sources,
        concat_ws(
            chr(31),
            {address_composite}
        ) as _address_composite
    from members
),
{attr_block},
{decision_block},
grid as (
{grid_block}
)
select
    g.entity_id,
    cast(g.attribute as VARCHAR) as attribute,
    cast(g.record_key as VARCHAR) as record_key,
    cast(r.source_system as VARCHAR) as source_system,
    cast(r.source_record_id as VARCHAR) as source_record_id,
    cast(g.rule as VARCHAR) as rule,
    cast('{version}' as VARCHAR) as survivorship_version
from grid as g
join {std_table} as r on r.record_key = g.record_key"
    ))
}
