{#
  S4.6 `golden_lineage` -- one row per `(entity_id, attribute)` saying WHICH record
  supplied the golden value and WHICH rule decided it.

  **The grid is complete.** Six rows per entity, one per token of S4.6's closed
  vocabulary `{email, phone_e164, given_name, family_name, address, birth_date}`, emitted
  even when the winning value is NULL. A model that dropped NULL winners would leave
  `(entity_id, attribute)` a key with holes, would give ER-090's expected file a shape
  that varied by fixture, and would make AC3's `6 * count(distinct entity_id)` fail like
  a data problem rather than a modelling one. The winning RECORD is never null: an entity
  exists because it holds members, and a member has a `record_key` whatever its values.

  **Eleven columns, six decisions.** The six `addr_*` columns are one decision under the
  token `address` (S4.6), so the vocabulary is shorter than the survivable column set by
  exactly the composite. `src/er/lake/columns.py::GOLDEN_LINEAGE_ATTRIBUTES` derives one
  from the other rather than listing both.

  **The rule vocabulary is closed by the macro, not by this model.** ER-087's
  `survivorship_decision` returns either a rule name from the chain or
  `tiebreak_deterministic` -- the terminal `record_key ASC` having decided, or there
  having been a single candidate with nothing to decide against. `schema.yml`'s
  `accepted_values` test is what holds that closed; this model only passes it through.

  **Why this re-runs the decisions rather than reading `golden_records`.** The winner's
  `record_key` is not a column of `golden_records` -- it is the thing this relation
  exists to record. Both models render the same `survivorship_decision` over the same
  `attr_*` projections from the same `int_std_records` rows, so they agree by
  construction; the alternative, joining `golden_records` back to `int_std_records` on
  matching VALUES, would pick the wrong record whenever two members share a value, which
  is most of the time.
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

{%- for attribute in scalar_attributes %}
, decision_{{ attribute }} as (
    {{ survivorship_decision(attribute, chains[attribute], 'attr_' ~ attribute) }}
)
{%- endfor %}

, decision_address as (
    {{ survivorship_decision('address', chains['address'], 'attr_address') }}
)

{#- The grid, one arm per vocabulary token. `attribute` is a literal per arm rather
    than a pivot, so a token added to S4.6 is a compile error here instead of a row
    quietly missing from every entity. -#}
, grid as (

    {%- for attribute in scalar_attributes + ['address'] %}
    select
        entity_id,
        '{{ attribute }}' as attribute,
        record_key,
        rule
    from decision_{{ attribute }}
    {{ "union all" if not loop.last }}
    {%- endfor %}

)

select
    g.entity_id,
    cast(g.attribute as VARCHAR) as attribute,
    cast(g.record_key as VARCHAR) as record_key,
    cast(r.source_system as VARCHAR) as source_system,
    cast(r.source_record_id as VARCHAR) as source_record_id,
    cast(g.rule as VARCHAR) as rule,
    cast('{{ var('survivorship_version') }}' as VARCHAR) as survivorship_version,
    cast('{{ run_started_at.strftime('%Y-%m-%d %H:%M:%S') }}' as TIMESTAMP) as assembled_at
from grid as g
join {{ ref('int_std_records') }} as r
  on r.record_key = g.record_key
