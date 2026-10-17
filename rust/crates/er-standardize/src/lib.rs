//! The standardization stage (port of the retired dbt models `stg_records.sql`
//! + `int_std_records.sql`, DesignDoc.md S4.2).
//!
//! `stg_records` projects `raw_records.payload` into the canonical staging shape
//! by applying the [`er_std::sql`] macros per source; `int_std_records` keeps one
//! current row per `(source_system, source_record_id)` (greatest `ingested_at`,
//! tie-broken by `ingest_batch_id DESC`) and drops tombstones. Both are assembled
//! as DuckDB SQL and run through a dedupe-core session; `stg_records` exists only
//! as a CTE inside the `int_std_records` statement, never as a lake relation.

use dedupe_core::session::Session;
use er_config::{Config, SourceSpec};
use er_std::sql::{
    address_parse, company_name, company_name_norm, company_variants, domain, domain_norm,
    email_norm, lowercase_trim, name_norm, name_variants, null_semantics, parse_date, phone_e164,
    source_metadata,
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

/// `json_extract_string(payload, '<col>')` — a VARCHAR accessor into the JSON
/// payload. The named-function spelling of `payload ->> '<col>'` (value-identical
/// in DuckDB), so the analyser's plan-vector tests resolve it through the ff
/// json stubs without an operator rewrite.
fn field(col: &str) -> String {
    format!("json_extract_string(payload, '{}')", single_quote(col))
}

/// The keys `source_metadata` excludes: the record-id and updated-at columns,
/// reserved `persona_id`, and every mapped canonical column. Entity-agnostic:
/// `spec.columns.values()` is exactly the mapped canonical source columns for
/// whichever entity type the source belongs to (person nine or Account seven), so
/// a person spec yields the identical set as before — `source_metadata` uses this
/// only as a membership filter, so order does not matter.
fn metadata_excluded(spec: &SourceSpec) -> Vec<String> {
    let mut excluded = vec![
        spec.record_id_column.clone(),
        spec.updated_at_column.clone(),
        "persona_id".to_string(),
    ];
    excluded.extend(spec.columns.values().cloned());
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

// ---- Account entity type (1.1b) --------------------------------------------
// A parallel standardization projection for `entity_type: account`. Additive:
// person functions above are untouched; `run_standardize` picks this path only
// when `config.entity_type == Account`. Mirrors the person shape (supersession +
// tombstone + computed record_key) over the Account attribute set, writing
// `acct_int_std_records` (er-schema `acct_std_payload`). No lexicon — company
// variants carry no synonym expansion.

/// The `acct_int_std_records` payload columns, in S5 order (mirror of
/// `er_schema::model::acct_std_payload`; `record_key` is computed + prepended).
pub const ACCT_STD_PAYLOAD: [&str; 19] = [
    "source_system",
    "source_record_id",
    "content_hash",
    "std_version",
    "company_name",
    "company_name_norm",
    "company_variants",
    "domain",
    "domain_norm",
    "phone_e164",
    "phone_valid",
    "addr_street",
    "addr_city",
    "addr_region",
    "addr_postal",
    "metadata",
    "updated_at_source",
    "ingest_batch_id",
    "ingested_at",
];

/// `null_semantics(lowercase_trim(field))` — the directly-mapped Account address
/// columns (no `address_line` parse; the Account schema has no addr_number/unit).
fn addr_passthrough(col: &str) -> String {
    null_semantics(&lowercase_trim(col))
}

/// One Account `stg_records` union arm: projects `raw_records` through the Account
/// S4.2 macros into the Account staging shape.
fn acct_stg_arm(source: &str, spec: &SourceSpec, config: &Config, raw_table: &str) -> String {
    let std_version = single_quote(&config.versions.std_version);
    let region = &config.standardization.phone_default_region;
    let excluded = metadata_excluded(spec);
    let excluded_refs: Vec<&str> = excluded.iter().map(String::as_str).collect();
    let company = field(&spec.columns["company_name"]);
    format!(
        "select\n    source_system,\n    source_record_id,\n    content_hash,\n    '{std_version}' as std_version,\n    \
         {} as company_name,\n    {} as company_name_norm,\n    {} as company_variants,\n    \
         {} as domain,\n    {} as domain_norm,\n    {},\n    \
         {} as addr_street,\n    {} as addr_city,\n    {} as addr_region,\n    {} as addr_postal,\n    \
         {} as metadata,\n    try_cast({} as timestamp) as updated_at_source,\n    \
         ingest_batch_id,\n    ingested_at\nfrom {raw_table}\nwhere source_system = '{}'",
        company_name(&company),
        company_name_norm(&company),
        company_variants(&company),
        domain(&field(&spec.columns["domain"])),
        domain_norm(&field(&spec.columns["domain"])),
        phone_e164(&field(&spec.columns["phone"]), region),
        addr_passthrough(&field(&spec.columns["addr_street"])),
        addr_passthrough(&field(&spec.columns["addr_city"])),
        addr_passthrough(&field(&spec.columns["addr_region"])),
        addr_passthrough(&field(&spec.columns["addr_postal"])),
        source_metadata("payload", &excluded_refs),
        field(&spec.updated_at_column),
        single_quote(source),
    )
}

/// The Account `stg_records` SQL: one union arm per source, sorted source order.
#[must_use]
pub fn acct_stg_records_sql(config: &Config, raw_table: &str) -> String {
    let mut sources: Vec<&String> = config.sources.keys().collect();
    sources.sort();
    sources
        .iter()
        .map(|source| acct_stg_arm(source, &config.sources[*source], config, raw_table))
        .collect::<Vec<_>>()
        .join("\nunion all\n")
}

/// The `acct_int_std_records` SQL: supersession + tombstone + computed record_key
/// over the Account staging rows (mirror of [`int_std_records_sql`]).
#[must_use]
pub fn acct_int_std_records_sql(config: &Config, raw_table: &str) -> String {
    let payload = ACCT_STD_PAYLOAD.join(", ");
    let stg = acct_stg_records_sql(config, raw_table);
    format!(
        "with versions as (\n{stg}\n),\n\
         current_version as (\n  select {payload} from versions\n  \
         qualify row_number() over (partition by source_system, source_record_id \
         order by ingested_at desc, ingest_batch_id desc) = 1\n)\n\
         select source_system || ':' || source_record_id as record_key, {payload}\n\
         from current_version\nwhere content_hash <> '{TOMBSTONE_CONTENT_HASH}'"
    )
}

/// Materialise `acct_int_std_records` into `out_table` from the landed `raw_table`.
pub fn build_acct_int_std_records(
    session: &Session,
    config: &Config,
    raw_table: &str,
    out_table: &str,
) -> anyhow::Result<()> {
    let sql = acct_int_std_records_sql(config, raw_table);
    session
        .execute_batch(&format!("create table {out_table} as\n{sql}"))
        .map_err(|e| anyhow::anyhow!("build {out_table}: {e}"))?;
    Ok(())
}
