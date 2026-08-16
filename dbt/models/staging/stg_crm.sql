{#
  S4.2 `stg_crm` -- one of v1's three hand-written staging models.

  It projects `raw_records.payload` into the canonical `stg_<source>` shape S5
  declares, applying the S4.2 standardization macros. The source column names are
  NOT written here: they come from `var('sources')['crm']['columns']`, the S6
  document the CLI passes on every invocation (`render_dbt_vars`), so adding a
  source is a config change plus one model file and a column rename is a config
  change alone.

  The projection order is S5's column order and the contract in `schema.yml`
  enforces it. Three of the macros expand to more than one projection --
  `email_norm` to two, `phone_e164` to two, `address_parse` to six -- and they
  carry their own aliases, which is why those lines have none: the alias is part
  of the S5 column contract and this model must not restate it. `persona_id` is a
  fixture bookkeeping column and is named by no `columns:` mapping, so it cannot
  reach a staged row (S8.2).

  Rows are staged as delivered, tombstones included: S4.2 excludes a tombstoned
  record in `int_std_records`, not here, so that `stg_*` stays a faithful
  projection of the version history `raw_records` holds.

  The incremental block is normative and identical on all three models. `append`
  with the *not-in-distinct-batch* predicate -- never a `>` watermark on
  `ingested_at` -- is what makes `er standardize` idempotent (M16): a re-delivered
  or replayed batch is recognised by its id and contributes nothing, while a
  watermark would re-append every row of a batch whose stamps tie.

  `on_schema_change` is `append_new_columns`, which is what propagates the additive
  column a `std_version` bump introduces (S4.2, S5.1). It is spelled on the model
  as well as at the `dbt_project.yml` root because S4.2 states it per model family,
  and dbt-core refuses every other value but `fail` once the S5.0 contract is
  enforced on an incremental model.
#}
{{ config(materialized='incremental', incremental_strategy='append',
          on_schema_change='append_new_columns') }}

{%- set source_system = 'crm' -%}
{%- set spec = var('sources')[source_system] -%}

{#- One JSON accessor per canonical attribute, built once so the quoting of a
    source column name happens in exactly one place. `payload` is JSON (S5) and
    `->>` yields VARCHAR, which is what every S4.2 macro takes. -#}
{%- set field = {} -%}
{%- for canonical, source_column in spec['columns'].items() -%}
  {%- do field.update({canonical: "payload ->> '" ~ source_column | replace("'", "''") ~ "'"}) -%}
{%- endfor -%}
{%- set updated_at = "payload ->> '" ~ spec['updated_at_column'] | replace("'", "''") ~ "'" -%}

select
    source_system,
    source_record_id,
    content_hash,
    '{{ var('std_version') }}' as std_version,
    {{ name_norm(field['given_name']) }} as given_name,
    {{ name_norm(field['family_name']) }} as family_name,
    {{ name_variants(field['given_name']) }} as name_variants,
    {{ email_norm(field['email']) }},
    {{ phone_e164(field['phone']) }},
    {{ address_parse(
        field['address_line'], field['addr_city'], field['addr_region'], field['addr_postal']
    ) }},
    {{ parse_date(field['birth_date'], spec['date_format']) }} as birth_date,
    try_cast({{ updated_at }} as timestamp) as updated_at_source,
    ingest_batch_id,
    ingested_at
from {{ source('lake', 'raw_records') }}
where source_system = '{{ source_system }}'
{% if is_incremental() %}
  and ingest_batch_id not in (select distinct ingest_batch_id from {{ this }})
{% endif %}
