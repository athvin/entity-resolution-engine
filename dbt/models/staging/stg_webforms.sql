{#
  S4.2 `stg_webforms` -- the third of v1's three hand-written staging models.

  Identical in shape to `stg_crm`, which carries the commentary this one does not
  repeat; the only differences are the source it filters and the S6 entry it reads
  its column mapping from.
#}
{{ config(materialized='incremental', incremental_strategy='append',
          on_schema_change='append_new_columns') }}

{%- set source_system = 'webforms' -%}
{%- set spec = var('sources')[source_system] -%}

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
