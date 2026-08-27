{#
  S4.6 `golden_display` -- presentation casing on top of `golden_records`, and nothing
  else.

  **Its defining property is negative.** S4.6: "presentation casing only and is never
  read by the matching layer, so matching-layer data is never re-cased". A model cannot
  assert that about itself, so `tests/unit/test_golden_display_isolation.py` is the guard
  -- a source scan over the matching, entity, ingest and staging/intermediate trees, plus
  a dbt-manifest check that this node has zero children. This file's job is to give that
  guard something true to protect.

  **It reads `golden_records` and NOTHING else.** Reaching back to `int_std_records`
  would re-introduce a second survivorship path: two models deciding which record wins,
  agreeing on every fixture until the day they did not. The survivorship decision was
  made once, upstream; this model only re-renders its output.

  **It carries no `survivorship_version`, deliberately.** S4.6 says its provenance is
  read by joining `golden_records` and `golden_lineage` on `entity_id`. A column added
  here "for symmetry" would be a second answer to a question those two already answer,
  and the second answer is the one that goes stale.

  **The four transforms are pinned, not configurable.** v1 has no locale or i18n
  configuration and S4.6 asks for none, so each is fixed here and asserted with a
  positive and a NULL-handling case:

    display_name    title-cased `given_name` and `family_name`, single space, NULL parts
                    dropped -- so a missing given name yields no leading space.
    display_phone   `(NNN) NNN-NNNN` for a +1 NANP number, the E.164 string VERBATIM
                    otherwise. An international number passes through rather than being
                    mangled into a US shape.
    display_address the six `addr_*` composed as `number street unit, city region
                    postal`, with empty parts and their separators dropped, so no
                    address renders with a dangling comma or a double space.
    display_email   `golden_records.email`, byte for byte. The display layer never
                    re-cases an email: `email_norm` already decided its form (S4.2), and
                    a second opinion here would make the displayed address differ from
                    the one that was matched on.
#}

{{ config(materialized='incremental', incremental_strategy='delete+insert',
          unique_key='entity_id',
          on_schema_change='append_new_columns') }}

{#- Title case, per word, as a template rather than a macro: dbt registers macros
    only from `macros/`, so a `{% macro %}` written inside a model compiles to
    "'title_case' is undefined". One definition, substituted per column below.

    DuckDB has no `initcap`. Capitalising each whitespace-separated word and lower-casing
    the rest turns `mary jane WATSON` into `Mary Jane Watson`; the shorter
    `upper(substr(x,1,1)) || lower(substr(x,2))` would give `Mary jane watson`, which is
    right for a single-word name and wrong for every other. `trim` runs first so a padded
    value does not split into an empty leading word. -#}
{%- set TITLE_CASE -%}
array_to_string(list_transform(string_split(trim(__COLUMN__), ' '), w -> upper(substr(w, 1, 1)) || lower(substr(w, 2))), ' ')
{%- endset -%}

{%- set address_columns = [
    'addr_number', 'addr_street', 'addr_unit', 'addr_city', 'addr_region', 'addr_postal'
] -%}

with rendered as (

    select
        entity_id,

        {#- NULL parts dropped rather than coalesced to '': concat_ws skips NULLs but
            would keep an empty string as a separator run. -#}
        nullif(
            concat_ws(
                ' ',
                nullif({{ TITLE_CASE | replace('__COLUMN__', 'given_name') }}, ''),
                nullif({{ TITLE_CASE | replace('__COLUMN__', 'family_name') }}, '')
            ),
            ''
        ) as display_name,

        email as display_email,

        {#- NANP only: +1 followed by exactly ten digits. Anything else -- a +44 number,
            a short code, a NULL -- is returned as it stands. -#}
        case
            when phone_e164 is null then null
            when regexp_matches(phone_e164, '^\+1[0-9]{10}$')
                then '(' || substr(phone_e164, 3, 3) || ') '
                     || substr(phone_e164, 6, 3) || '-'
                     || substr(phone_e164, 9, 4)
            else phone_e164
        end as display_phone,

        {#- `number street unit, city region postal`. Each group is composed first, so a
            group that is entirely empty takes its comma with it. -#}
        nullif(
            concat_ws(
                ', ',
                nullif(
                    concat_ws(
                        ' ',
                        nullif(trim(addr_number), ''),
                        nullif(trim(addr_street), ''),
                        nullif(trim(addr_unit), '')
                    ),
                    ''
                ),
                nullif(
                    concat_ws(
                        ' ',
                        nullif(trim(addr_city), ''),
                        nullif(trim(addr_region), ''),
                        nullif(trim(addr_postal), '')
                    ),
                    ''
                )
            ),
            ''
        ) as display_address,

        assembled_at

    from {{ ref('golden_records') }}

)

select
    entity_id,
    cast(display_name as VARCHAR) as display_name,
    cast(display_email as VARCHAR) as display_email,
    cast(display_phone as VARCHAR) as display_phone,
    cast(display_address as VARCHAR) as display_address,
    assembled_at
from rendered
