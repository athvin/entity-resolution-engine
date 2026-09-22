{# Batch identity, not a timestamp watermark, defines unfinished work. Each
   affected record is resolved across all its staged versions, including tombstones. #}
{% macro standardize_key_filter(source_column, id_column) -%}
{% if var('standardize_delta', false) %}
exists (
    select 1
    from {{ source('lake', 'raw_records') }} as pending_raw
    join {{ source('lake', 'er_standardize_work') }} as pending_work
      on pending_work.source_system = pending_raw.source_system
     and pending_work.ingest_batch_id = pending_raw.ingest_batch_id
    where pending_work.run_id = '{{ var('run_id') }}'
      and pending_raw.source_system = {{ source_column }}
      and pending_raw.source_record_id = {{ id_column }}
)
{% else %}true{% endif %}
{%- endmacro %}

{% macro remove_changed_blocking_keys() -%}
{% if is_incremental() %}
delete from {{ this }} as old_keys
where {{ standardize_key_filter('old_keys.source_system', 'old_keys.source_record_id') }}
{% else %}select 1{% endif %}
{%- endmacro %}
