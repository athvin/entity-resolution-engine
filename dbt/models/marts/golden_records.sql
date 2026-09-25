{# The lineage model selects each winner once. Values are read from those same
   records, so assembly cannot disagree with its own explanation. #}
{{ config(materialized='incremental', incremental_strategy='delete+insert',
          unique_key='entity_id', on_schema_change='append_new_columns') }}

{%- set scalar_attributes = ['given_name', 'family_name', 'email', 'phone_e164', 'birth_date'] -%}
{%- set address_columns = ['addr_number', 'addr_street', 'addr_unit', 'addr_city', 'addr_region', 'addr_postal'] -%}

with source_metadata as (
    select
        m.entity_id,
        r.source_system,
        to_json(map_from_entries(list(
            struct_pack(key := r.source_record_id, value := r.metadata)
            order by r.source_record_id
        ))) as metadata
    from {{ source('lake', 'entity_membership') }} as m
    join {{ ref('int_std_records') }} as r on r.record_key = m.record_key
    join {{ source('lake', 'entities') }} as e on e.entity_id = m.entity_id and e.status = 'active'
    where {{ assemble_entity_filter('m.entity_id') }}
      and coalesce(array_length(json_keys(r.metadata)), 0) > 0
    group by m.entity_id, r.source_system
),
entity_metadata as (
    select entity_id,
           to_json(map_from_entries(list(
               struct_pack(key := source_system, value := metadata) order by source_system
           ))) as metadata
    from source_metadata
    group by entity_id
),
survived as (
select
    l.entity_id,
    {%- for attribute in scalar_attributes %}
    max(r.{{ attribute }}) filter (where l.attribute = '{{ attribute }}') as {{ attribute }},
    {%- endfor %}
    {%- for column in address_columns %}
    max(r.{{ column }}) filter (where l.attribute = 'address') as {{ column }},
    {%- endfor %}
    cast('{{ var('survivorship_version') }}' as VARCHAR) as survivorship_version,
    cast('{{ var('run_started_at', run_started_at.strftime('%Y-%m-%d %H:%M:%S')) }}' as TIMESTAMP) as assembled_at
from {{ ref('golden_lineage') }} as l
join {{ ref('int_std_records') }} as r on r.record_key = l.record_key
join {{ source('lake', 'entities') }} as e on e.entity_id = l.entity_id and e.status = 'active'
where {{ assemble_entity_filter('l.entity_id') }}
group by l.entity_id
)
select s.*, coalesce(m.metadata, '{}'::json) as metadata
from survived as s
left join entity_metadata as m using (entity_id)
