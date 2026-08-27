{#
  S4.6 `golden_records` -- one row per active entity, each attribute decided by its own
  S6 survivorship chain.

  **The shape ER-087's macro requires, and why this model is the one that presents it.**
  `survivorship_decision(attribute, chain, relation)` ranks a LONG relation: one row per
  `(entity_id, attribute, value)` carrying `record_key`, `source_system`, `sources`,
  `updated_at_source`, `ingested_at` and `<attr>_valid`. `rules.sql` names this model as
  the presenter of those columns, so the `attr_*` CTEs below are that contract written
  out. The `sources` MAP is built from the `sources` var rather than from a seed because
  `source_priority` reads `sources[source_system].priority_rank`, and S6 is the one place
  a priority is declared (M26).

  **The address is ONE decision, not six.** S4.6: "when `address` wins, all six `addr_*`
  columns MUST come from the single winning contributing record, never assembled
  field-by-field across records". So `address` is decided once -- over a composite value
  standing in for the whole address -- and the six columns are then read off the winning
  record by joining `int_std_records` on that one `record_key`. Six independent decisions
  would agree with this on any fixture whose members share an address and diverge the
  moment one does not, which is exactly the defect the rule names and is invisible to a
  test that only compares values.

  The composite exists so `completeness` and `frequency` have something to measure --
  "this record has the fullest address", "three records agree on this address" -- which
  they cannot do against a column that does not exist. Its separator is US (U+001F), a
  control character no standardized address component can contain, so two different
  addresses can never collide into one composite and be counted as agreeing.

  **`assembled_at` is the run's stamp, and every active entity holding a member is
  built.** Touched-only assembly, `er_touched_entities` and the reap step are ER-092's.
  Building the full corpus is what makes S4.6's "every active entity with >= 1 member has
  exactly one golden row" checkable today.
#}

{{ config(materialized='incremental', incremental_strategy='delete+insert',
          unique_key='entity_id',
          on_schema_change='append_new_columns') }}

{%- set address_columns = [
    'addr_number', 'addr_street', 'addr_unit', 'addr_city', 'addr_region', 'addr_postal'
] -%}
{%- set scalar_attributes = [
    'given_name', 'family_name', 'email', 'phone_e164', 'birth_date'
] -%}
{#- The `validated` rule's input column per attribute. `int_std_records`
    carries `email_valid` and `phone_valid`; S4.6 writes the fragment as
    `<attr>_valid`, so phone's two spellings are reconciled by the alias
    below rather than by a special case inside the rule. -#}
{%- set validated_source = {'email': 'email_valid', 'phone_e164': 'phone_valid'} -%}
{%- set validated_attributes = validated_source.keys() | list -%}
{%- set chains = var('survivorship') -%}

with members as (

    select
        m.entity_id,
        r.*
    from {{ ref('int_std_records') }} as r
    join {{ source('lake', 'entity_membership') }} as m
      on m.record_key = r.record_key
    join {{ source('lake', 'entities') }} as e
      on e.entity_id = m.entity_id
     and e.status = 'active'

),

member_rows as (

    select
        *,
        MAP {
            {%- for name, spec in var('sources').items() %}
            '{{ name }}': struct_pack(priority_rank := {{ spec.get('priority_rank', 999) }})
            {{- "," if not loop.last }}
            {%- endfor %}
        } as _sources,
        concat_ws(
            chr(31)
            {%- for column in address_columns %},
            coalesce({{ column }}, '')
            {%- endfor %}
        ) as _address_composite
    from members

)

{#- The LONG projections, one per attribute: `value` aliased from the column the
    attribute names, `<attr>_valid` carried only where a chain can ask for it. -#}
{%- for attribute in scalar_attributes %}
, attr_{{ attribute }} as (
    select
        entity_id, record_key, source_system, source_record_id,
        updated_at_source, ingested_at,
        _sources as sources,
        {{ attribute }} as value
        {%- if attribute in validated_attributes %},
        {#- S4.6's `validated` fragment names `<attr>_valid`, and `int_std_records`
            spells phone's flag `phone_valid` rather than `phone_e164_valid`. Aliasing
            here is the presenter's job: `rules.sql` states that a rule names the
            columns its caller must present and that this model is what presents them,
            so the alias belongs here and the macro stays literal. -#}
        {{ validated_source[attribute] }} as {{ attribute }}_valid
        {%- endif %}
    from member_rows
)
{%- endfor %}

, attr_address as (
    select
        entity_id, record_key, source_system, source_record_id,
        updated_at_source, ingested_at,
        _sources as sources,
        _address_composite as value
    from member_rows
)

{#- One ranking per attribute, through the S6 chain. Every decision CTE is defined
    after the projection it reads: DuckDB resolves a WITH list in order. -#}
{%- for attribute in scalar_attributes %}
, decision_{{ attribute }} as (
    {{ survivorship_decision(attribute, chains[attribute], 'attr_' ~ attribute) }}
)
{%- endfor %}

, decision_address as (
    {{ survivorship_decision('address', chains['address'], 'attr_address') }}
)

{#- The six address columns, read off the ONE record `decision_address` named. This join
    IS the composite rule: exactly one `record_key` per entity reaches it, so no field
    can come from anywhere else. -#}
, address_values as (

    select
        d.entity_id,
        {%- for column in address_columns %}
        r.{{ column }}{{ "," if not loop.last }}
        {%- endfor %}
    from decision_address as d
    join {{ ref('int_std_records') }} as r
      on r.record_key = d.record_key

)

select
    e.entity_id,
    {%- for attribute in scalar_attributes %}
    {%- if attribute == 'birth_date' %}
    cast(d_{{ attribute }}.value as DATE) as {{ attribute }},
    {%- else %}
    cast(d_{{ attribute }}.value as VARCHAR) as {{ attribute }},
    {%- endif %}
    {%- endfor %}
    {%- for column in address_columns %}
    cast(a.{{ column }} as VARCHAR) as {{ column }},
    {%- endfor %}
    cast('{{ var('survivorship_version') }}' as VARCHAR) as survivorship_version,
    cast('{{ run_started_at.strftime('%Y-%m-%d %H:%M:%S') }}' as TIMESTAMP) as assembled_at
from (select distinct entity_id from member_rows) as e
{%- for attribute in scalar_attributes %}
left join decision_{{ attribute }} as d_{{ attribute }}
       on d_{{ attribute }}.entity_id = e.entity_id
{%- endfor %}
left join address_values as a
       on a.entity_id = e.entity_id
