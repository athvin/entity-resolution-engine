{# Preserve unmapped source fields verbatim. JSON pointers keep punctuation in
   field names literal; sorted keys make repeated deliveries deterministic. #}
{% macro source_metadata(payload, spec) -%}
{%- set canonical = ['given_name', 'family_name', 'email', 'phone', 'address_line',
                     'addr_city', 'addr_region', 'addr_postal', 'birth_date'] -%}
{%- set excluded = [spec['record_id_column'], spec['updated_at_column'], 'persona_id'] -%}
{%- for attribute in canonical -%}
  {%- set _ = excluded.append(spec['columns'][attribute]) -%}
{%- endfor -%}
{%- set literals = excluded | map('replace', "'", "''") | join("', '") -%}
to_json(map_from_entries(list_transform(
    list_sort(list_filter(coalesce(json_keys({{ payload }}), []::varchar[]),
                          k -> not list_contains(['{{ literals }}'], k))),
    k -> struct_pack(key := k, value := json_extract(
        {{ payload }}, '/' || replace(replace(k, '~', '~0'), '/', '~1')))
)))
{%- endmacro %}
