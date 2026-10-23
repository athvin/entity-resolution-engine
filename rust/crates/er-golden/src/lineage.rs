//! `golden_lineage` (S4.6): one row per `(entity_id, attribute)` naming WHICH
//! member record supplied the golden value and WHICH rule decided it. A Rust
//! port of the retired dbt model `golden_lineage.sql` in full-assembly mode
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

/// Account scalar attributes (each its own golden_lineage row). The Account
/// analogue of [`SCALAR_ATTRIBUTES`]; `entity_type: account` only.
pub const ACCT_SCALAR_ATTRIBUTES: [&str; 3] = ["company_name", "domain", "phone_e164"];

/// The four Account `addr_*` columns collapsed under the `address` token (one
/// decision). Account has no `addr_number`/`addr_unit` (see `acct_std_payload`).
pub const ACCT_ADDRESS_COLUMNS: [&str; 4] =
    ["addr_street", "addr_city", "addr_region", "addr_postal"];

/// One field set that survives together, resolved for a config.
///
/// `token` is the `golden_lineage.attribute` the group's single decision is
/// recorded under; `fields` are the standardized columns taken from the winning
/// record; `controlling` is the field the chain ranks on, or `None` to rank on
/// the concatenation of every field.
pub struct ResolvedGroup<'a> {
    pub token: &'a str,
    pub fields: Vec<&'a str>,
    pub controlling: Option<&'a str>,
}

/// The groups in effect for `config`: the built-in `address` unless the config
/// declares its own, plus any others it declares.
///
/// An EMPTY `survivorship_groups` block must resolve to exactly the built-in
/// address group, in the position it has always occupied, because the generated
/// SQL is what the golden fixtures pin. That is the whole determinism
/// obligation of 1.6c and it is why the default is expressed here rather than
/// left to the caller.
#[must_use]
pub fn resolved_groups(config: &Config, account: bool) -> Vec<ResolvedGroup<'_>> {
    // The VOCABULARY comes from the caller, not from `config.entity_type`. Each
    // builder generates for one entity type by construction — `acct_*` is the
    // Account one — and taking it from the config instead would let a config
    // whose `entity_type` is defaulted or wrong generate person columns into
    // Account SQL, which is a planning failure a long way from its cause.
    let builtin: &[&str] = if account {
        &ACCT_ADDRESS_COLUMNS
    } else {
        &ADDRESS_COLUMNS
    };
    let mut groups: Vec<ResolvedGroup<'_>> = Vec::new();
    if !config.survivorship_groups.contains_key("address") {
        groups.push(ResolvedGroup {
            token: "address",
            fields: builtin.to_vec(),
            controlling: None,
        });
    }
    // BTreeMap order, so the generated SQL does not depend on YAML key order.
    for (token, group) in &config.survivorship_groups {
        groups.push(ResolvedGroup {
            token,
            fields: group.fields.iter().map(String::as_str).collect(),
            controlling: group.controlling.as_deref(),
        });
    }
    groups.sort_by_key(|g| g.token);
    groups
}

/// The scalar attributes still elected INDEPENDENTLY for `config` — the
/// vocabulary minus every field a group claimed.
///
/// A field cannot be both: two decisions writing the same column is the
/// ambiguity groups exist to remove, and silently preferring one would be the
/// dead-config shape this repo refuses.
#[must_use]
pub fn resolved_scalars(config: &Config, account: bool) -> Vec<&'static str> {
    // Caller-chosen, for the same reason as `resolved_groups`.
    let vocabulary: &[&'static str] = if account {
        &ACCT_SCALAR_ATTRIBUTES
    } else {
        &SCALAR_ATTRIBUTES
    };
    let claimed: std::collections::BTreeSet<&str> = config
        .survivorship_groups
        .values()
        .flat_map(|g| g.fields.iter().map(String::as_str))
        .collect();
    vocabulary
        .iter()
        .copied()
        .filter(|a| !claimed.contains(*a))
        .collect()
}

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

/// Account validity flags: only `phone_e164` carries one (`phone_valid`);
/// `company_name`/`domain` have none.
fn acct_validated_column(attribute: &str) -> Option<&'static str> {
    match attribute {
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
    run_at: &str,
) -> Result<String> {
    let version = config.versions.survivorship_version.replace('\'', "''");
    let run_at = run_at.replace('\'', "''");

    // The field sets that survive together (1.6c). With no declared groups this
    // is exactly the built-in `address`, which is what keeps the generated SQL —
    // and so every golden fixture — byte-identical for an existing config.
    let groups = resolved_groups(config, false);
    let scalars = resolved_scalars(config, false);

    // One composite column per group that ranks on its whole value rather than
    // on a controlling field. `chr(31)` (unit separator) joins the parts, so a
    // value containing a comma cannot forge a different composite.
    let mut member_selects: Vec<String> = vec![format!("{} as _sources", sources_map(config))];
    for group in groups.iter().filter(|g| g.controlling.is_none()) {
        let parts = group
            .fields
            .iter()
            .map(|c| format!("coalesce({c}, '')"))
            .collect::<Vec<_>>()
            .join(",\n            ");
        member_selects.push(format!(
            "concat_ws(\n            chr(31),\n            {parts}\n        ) as _{}_composite",
            group.token
        ));
    }

    // Per-attribute relations in LONG form: value + the keys every rule reads.
    let mut attr_ctes: Vec<String> = Vec::new();
    for attribute in &scalars {
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
    for group in &groups {
        // A controlling field ranks on ITS value; without one the chain ranks on
        // the concatenation, which is what `address` has always done —
        // `most_common` over an address picks the most common address, not the
        // most common street.
        let value = group.controlling.map_or_else(
            || format!("_{}_composite", group.token),
            ToString::to_string,
        );
        attr_ctes.push(format!(
            "attr_{} as (
    select
        entity_id, record_key, source_system, source_record_id,
        updated_at_source, ingested_at, _sources as sources,
        {value} as value
    from member_rows
)",
            group.token
        ));
    }

    // Each attribute's winner via its survivorship chain.
    let tokens: Vec<&str> = scalars
        .iter()
        .copied()
        .chain(groups.iter().map(|g| g.token))
        .collect();
    let mut decision_ctes: Vec<String> = Vec::new();
    for attribute in &tokens {
        let chain = config.survivorship.get(*attribute).ok_or_else(|| {
            anyhow::anyhow!("survivorship chain missing for attribute {attribute:?}")
        })?;
        let decision = survivorship_decision_sql(attribute, chain, &format!("attr_{attribute}"))?;
        decision_ctes.push(format!("decision_{attribute} as (\n{decision}\n)"));
    }

    // The grid: one arm per vocabulary token, a literal attribute per arm.
    let grid_arms: Vec<String> = tokens
        .iter()
        .map(|attribute| {
            format!(
                "    select entity_id, '{attribute}' as attribute, record_key, rule\n    \
                 from decision_{attribute}"
            )
        })
        .collect();

    let member_block = member_selects.join(",\n        ");
    let attr_block = attr_ctes.join(",\n");
    let decision_block = decision_ctes.join(",\n");
    let grid_block = grid_arms.join("\n    union all\n");

    Ok(format!(
        "with members as (
    select m.entity_id, r.*
    from {std_table} as r
    join {membership_table} as m on m.record_key = r.record_key
    join {entities_table} as e on e.id = m.entity_id and e.status = 'active'
),
member_rows as (
    select
        *,
        {member_block}
    from members
),
{attr_block},
{decision_block},
grid as (
{grid_block}
)
select
    sha256('golden_lineage:' || g.entity_id || ':' || g.attribute) as id,
    g.entity_id,
    cast(g.attribute as VARCHAR) as attribute,
    cast(g.record_key as VARCHAR) as record_key,
    cast(r.source_system as VARCHAR) as source_system,
    cast(r.source_record_id as VARCHAR) as source_record_id,
    cast(g.rule as VARCHAR) as rule,
    cast('{version}' as VARCHAR) as survivorship_version,
    cast('{run_at}' as timestamp) as created_at,
    cast('{run_at}' as timestamp) as updated_at
from grid as g
join {std_table} as r on r.record_key = g.record_key"
    ))
}

/// The Account `acct_golden_lineage` SELECT — parallel to [`golden_lineage_sql`]
/// with the Account vocabulary `{company_name, domain, phone_e164, address}`
/// (`address` collapses the four Account `addr_*` columns). The person builder
/// above is intentionally left byte-for-byte untouched so its golden fixtures do
/// not move; this is a deliberate parallel copy (one lake = one entity_type).
pub fn acct_golden_lineage_sql(
    config: &Config,
    std_table: &str,
    membership_table: &str,
    entities_table: &str,
    run_at: &str,
) -> Result<String> {
    let version = config.versions.survivorship_version.replace('\'', "''");
    let run_at = run_at.replace('\'', "''");

    // The field sets that survive together (1.6c). With no declared groups this
    // is exactly the built-in `address`, which is what keeps the generated SQL —
    // and so every golden fixture — byte-identical for an existing config.
    let groups = resolved_groups(config, true);
    let scalars = resolved_scalars(config, true);

    // One composite column per group that ranks on its whole value rather than
    // on a controlling field. `chr(31)` (unit separator) joins the parts, so a
    // value containing a comma cannot forge a different composite.
    let mut member_selects: Vec<String> = vec![format!("{} as _sources", sources_map(config))];
    for group in groups.iter().filter(|g| g.controlling.is_none()) {
        let parts = group
            .fields
            .iter()
            .map(|c| format!("coalesce({c}, '')"))
            .collect::<Vec<_>>()
            .join(",\n            ");
        member_selects.push(format!(
            "concat_ws(\n            chr(31),\n            {parts}\n        ) as _{}_composite",
            group.token
        ));
    }

    // Per-attribute relations in LONG form: value + the keys every rule reads.
    let mut attr_ctes: Vec<String> = Vec::new();
    for attribute in &scalars {
        let valid = acct_validated_column(attribute).map_or(String::new(), |col| {
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
    for group in &groups {
        // A controlling field ranks on ITS value; without one the chain ranks on
        // the concatenation, which is what `address` has always done —
        // `most_common` over an address picks the most common address, not the
        // most common street.
        let value = group.controlling.map_or_else(
            || format!("_{}_composite", group.token),
            ToString::to_string,
        );
        attr_ctes.push(format!(
            "attr_{} as (
    select
        entity_id, record_key, source_system, source_record_id,
        updated_at_source, ingested_at, _sources as sources,
        {value} as value
    from member_rows
)",
            group.token
        ));
    }

    // Each attribute's winner via its survivorship chain.
    let tokens: Vec<&str> = scalars
        .iter()
        .copied()
        .chain(groups.iter().map(|g| g.token))
        .collect();
    let mut decision_ctes: Vec<String> = Vec::new();
    for attribute in &tokens {
        let chain = config.survivorship.get(*attribute).ok_or_else(|| {
            anyhow::anyhow!("survivorship chain missing for attribute {attribute:?}")
        })?;
        let decision = survivorship_decision_sql(attribute, chain, &format!("attr_{attribute}"))?;
        decision_ctes.push(format!("decision_{attribute} as (\n{decision}\n)"));
    }

    // The grid: one arm per vocabulary token, a literal attribute per arm.
    let grid_arms: Vec<String> = tokens
        .iter()
        .map(|attribute| {
            format!(
                "    select entity_id, '{attribute}' as attribute, record_key, rule\n    \
                 from decision_{attribute}"
            )
        })
        .collect();

    let member_block = member_selects.join(",\n        ");
    let attr_block = attr_ctes.join(",\n");
    let decision_block = decision_ctes.join(",\n");
    let grid_block = grid_arms.join("\n    union all\n");

    Ok(format!(
        "with members as (
    select m.entity_id, r.*
    from {std_table} as r
    join {membership_table} as m on m.record_key = r.record_key
    join {entities_table} as e on e.id = m.entity_id and e.status = 'active'
),
member_rows as (
    select
        *,
        {member_block}
    from members
),
{attr_block},
{decision_block},
grid as (
{grid_block}
)
select
    sha256('acct_golden_lineage:' || g.entity_id || ':' || g.attribute) as id,
    g.entity_id,
    cast(g.attribute as VARCHAR) as attribute,
    cast(g.record_key as VARCHAR) as record_key,
    cast(r.source_system as VARCHAR) as source_system,
    cast(r.source_record_id as VARCHAR) as source_record_id,
    cast(g.rule as VARCHAR) as rule,
    cast('{version}' as VARCHAR) as survivorship_version,
    cast('{run_at}' as timestamp) as created_at,
    cast('{run_at}' as timestamp) as updated_at
from grid as g
join {std_table} as r on r.record_key = g.record_key"
    ))
}
