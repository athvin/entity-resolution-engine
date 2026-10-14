//! The standardization stage (port of `dbt/models/staging/stg_records.sql` +
//! `dbt/models/intermediate/int_std_records.sql`, DesignDoc.md S4.2).
//!
//! `stg_records` projects `raw_records.payload` into the canonical staging shape
//! by applying the [`er_std::sql`] macros per source; `int_std_records` keeps one
//! current row per `(source_system, source_record_id)` (greatest `ingested_at`,
//! tie-broken by `ingest_batch_id DESC`) and drops tombstones. Both are assembled
//! as DuckDB SQL (no dbt) and run through a dedupe-core session.

use dedupe_core::session::Session;
use er_config::{CANONICAL_ATTRIBUTES, Config, SourceSpec};
use er_std::sql::{
    address_parse, email_norm, name_norm, name_variants, parse_date, phone_e164, source_metadata,
};

/// The `int_std_records` payload columns, in S5 order (`record_key` is computed
/// in the final projection and prepended, so it is not in this list).
pub const STD_PAYLOAD: [&str; 22] = [
    "source_system",
    "source_record_id",
    "content_hash",
    "std_version",
    "given_name",
    "family_name",
    "name_variants",
    "email",
    "email_valid",
    "phone_e164",
    "phone_valid",
    "addr_number",
    "addr_street",
    "addr_unit",
    "addr_city",
    "addr_region",
    "addr_postal",
    "birth_date",
    "metadata",
    "updated_at_source",
    "ingest_batch_id",
    "ingested_at",
];

/// The S4.1.1 tombstone sentinel.
const TOMBSTONE_CONTENT_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

fn single_quote(value: &str) -> String {
    value.replace('\'', "''")
}

/// `payload ->> '<col>'` — a VARCHAR accessor into the JSON payload.
fn field(col: &str) -> String {
    format!("payload ->> '{}'", single_quote(col))
}

/// The keys `source_metadata` excludes: the record-id and updated-at columns,
/// reserved `persona_id`, and every mapped canonical column.
fn metadata_excluded(spec: &SourceSpec) -> Vec<String> {
    let mut excluded = vec![
        spec.record_id_column.clone(),
        spec.updated_at_column.clone(),
        "persona_id".to_string(),
    ];
    for attr in CANONICAL_ATTRIBUTES {
        excluded.push(spec.columns[attr].clone());
    }
    excluded
}

/// One `stg_records` union arm for a source: projects `raw_records` through the
/// S4.2 macros into the canonical staging shape.
fn stg_arm(
    source: &str,
    spec: &SourceSpec,
    config: &Config,
    raw_table: &str,
    lexicon: &str,
) -> String {
    let std_version = single_quote(&config.versions.std_version);
    let region = &config.standardization.phone_default_region;
    let strip_plus = config.standardization.email_strip_plus_addressing;
    let placeholders: Vec<&str> = config
        .standardization
        .email_placeholders
        .iter()
        .map(String::as_str)
        .collect();
    let excluded = metadata_excluded(spec);
    let excluded_refs: Vec<&str> = excluded.iter().map(String::as_str).collect();

    format!(
        "select\n    source_system,\n    source_record_id,\n    content_hash,\n    '{std_version}' as std_version,\n    \
         {} as given_name,\n    {} as family_name,\n    {} as name_variants,\n    {},\n    {},\n    {},\n    \
         {} as birth_date,\n    {} as metadata,\n    try_cast({} as timestamp) as updated_at_source,\n    \
         ingest_batch_id,\n    ingested_at\nfrom {raw_table}\nwhere source_system = '{}'",
        name_norm(&field(&spec.columns["given_name"])),
        name_norm(&field(&spec.columns["family_name"])),
        name_variants(&field(&spec.columns["given_name"]), lexicon),
        email_norm(&field(&spec.columns["email"]), strip_plus, &placeholders),
        phone_e164(&field(&spec.columns["phone"]), region),
        address_parse(
            &field(&spec.columns["address_line"]),
            &field(&spec.columns["addr_city"]),
            &field(&spec.columns["addr_region"]),
            &field(&spec.columns["addr_postal"]),
        ),
        parse_date(&field(&spec.columns["birth_date"]), &spec.date_format),
        source_metadata("payload", &excluded_refs),
        field(&spec.updated_at_column),
        single_quote(source),
    )
}

/// The `stg_records` SQL: one union arm per source, in sorted source order.
#[must_use]
pub fn stg_records_sql(config: &Config, raw_table: &str, lexicon: &str) -> String {
    let mut sources: Vec<&String> = config.sources.keys().collect();
    sources.sort();
    sources
        .iter()
        .map(|source| stg_arm(source, &config.sources[*source], config, raw_table, lexicon))
        .collect::<Vec<_>>()
        .join("\nunion all\n")
}

/// The `int_std_records` SQL over a `stg_records` subquery: supersession (one
/// current row per key) + tombstone exclusion + the computed `record_key`.
#[must_use]
pub fn int_std_records_sql(config: &Config, raw_table: &str, lexicon: &str) -> String {
    let payload = STD_PAYLOAD.join(", ");
    let stg = stg_records_sql(config, raw_table, lexicon);
    format!(
        "with versions as (\n{stg}\n),\n\
         current_version as (\n  select {payload} from versions\n  \
         qualify row_number() over (partition by source_system, source_record_id \
         order by ingested_at desc, ingest_batch_id desc) = 1\n)\n\
         select source_system || ':' || source_record_id as record_key, {payload}\n\
         from current_version\nwhere content_hash <> '{TOMBSTONE_CONTENT_HASH}'"
    )
}

/// Materialise `int_std_records` into `out_table` from the landed `raw_table`,
/// using the seeded `lexicon` relation for `name_variants`.
pub fn build_int_std_records(
    session: &Session,
    config: &Config,
    raw_table: &str,
    lexicon: &str,
    out_table: &str,
) -> anyhow::Result<()> {
    let sql = int_std_records_sql(config, raw_table, lexicon);
    session
        .execute_batch(&format!("create table {out_table} as\n{sql}"))
        .map_err(|e| anyhow::anyhow!("build {out_table}: {e}"))?;
    Ok(())
}
