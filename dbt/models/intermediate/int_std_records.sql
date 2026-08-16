{#
  S4.2 `int_std_records` -- the single current-state standardized relation.

  It unions the three `stg_<source>` models, materializes `record_key`,
  `content_hash`, `std_version` and `updated_at_source`, and applies the S4.2
  supersession rule: exactly ONE current row per `(source_system,
  source_record_id)`, taken from the `raw_records` version with the greatest
  `ingested_at`, ties broken by `ingest_batch_id DESC`. The ULID is time-ordered,
  so `DESC` selects the most recent batch -- `ASC` would let the older version
  win, which is M15 in one word.

  Rows whose winning version is a tombstone are excluded entirely (S4.1.1). The
  test is the sentinel `content_hash` a tombstone carries, and it is sound
  BECAUSE of the sentinel's own guarantee: the S4.1 hash function always hashes at
  least one separator-joined value, so it can never produce 64 zeros, and a
  content version can therefore never be mistaken for a tombstone. Exclusion is by
  the WINNING version's hash and not by any version having it, which is exactly
  what makes resurrection work with no special case -- a re-delivered key's
  ordinary content version has the greater `ingested_at`, wins, and the record is
  live again (S4.1.1).

  `record_key` is `source_system || ':' || source_record_id` (S5.0, D6): the
  canonical scalar identity, this relation's logical key, and Splink's
  `unique_id_column_name`. `':'` is banned from `source_record_id`, and
  `dbt/tests/assert_record_key_no_colon.sql` is where that ban is enforced.

  `phone_valid` travels beside `email_valid` because the `validated` survivorship
  rule needs an input for both attributes (S6.1 V4). Neither is a `golden_records`
  column (S5.0's `GOLDEN_SURVIVABLE_COLUMNS` names neither), and neither is
  survivable; they exist here and only here.

  **The incremental configuration is S4.2's, and the post-hook is what makes the
  tombstone arm true under it.** `delete+insert` keys its DELETE on the rows the
  model just produced, so a key the SELECT *excludes* is never named by that
  DELETE and its previous row would survive the run -- silently keeping a deleted
  record current, which is the one thing S4.2 says this relation may not do. The
  hook removes what the strategy structurally cannot: any row still carrying a
  version that a newer staged version supersedes. After the insert, every live key
  holds its winner, so a superseded row can only belong to a key this SELECT
  dropped. It is a no-op on a run with no tombstones, which is why AC7's
  re-standardize is still row-stable, and `assert_one_current_std_row_per_record`
  is the test that fails if it ever stops running.
#}

{#- The S5 `int_std_records` payload: `stg_<source>`'s column list, in S5's order.
    `record_key` is not in it -- the final projection computes it and puts it
    first, which is the order the contract in `schema.yml` enforces. -#}
{%- set std_payload = [
    'source_system',
    'source_record_id',
    'content_hash',
    'std_version',
    'given_name',
    'family_name',
    'name_variants',
    'email',
    'email_valid',
    'phone_e164',
    'phone_valid',
    'addr_number',
    'addr_street',
    'addr_unit',
    'addr_city',
    'addr_region',
    'addr_postal',
    'birth_date',
    'updated_at_source',
    'ingest_batch_id',
    'ingested_at'
] -%}

{%- set payload_projection -%}
{%- for column in std_payload %}
    {{ column }}{{ "," if not loop.last else "" }}
{%- endfor -%}
{%- endset -%}

{#- S4.1.1's tombstone sentinel. Spelled once here and asserted equal to
    `er.ingest.landing.TOMBSTONE_CONTENT_HASH` by the integration suite, because a
    dbt model cannot import the Python constant that produced the rows it reads. -#}
{%- set tombstone_content_hash = '0' * 64 -%}

{%- set drop_superseded_rows -%}
{% raw %}
-- S4.2: nothing superseded may remain current. `delete+insert` cannot express
-- this on its own -- see the header -- so the excluded tombstoned keys are
-- removed here, immediately after the insert this hook is attached to.
delete from {{ this }}
where record_key in (
    select current_row.record_key
    from {{ this }} as current_row
    join (
        select source_system, source_record_id, ingested_at, ingest_batch_id
        from {{ ref('stg_crm') }}
        union all
        select source_system, source_record_id, ingested_at, ingest_batch_id
        from {{ ref('stg_billing') }}
        union all
        select source_system, source_record_id, ingested_at, ingest_batch_id
        from {{ ref('stg_webforms') }}
    ) as version
      on version.source_system = current_row.source_system
     and version.source_record_id = current_row.source_record_id
    where version.ingested_at > current_row.ingested_at
       or (version.ingested_at = current_row.ingested_at
           and version.ingest_batch_id > current_row.ingest_batch_id)
)
{% endraw %}
{%- endset -%}

{{ config(materialized='incremental', incremental_strategy='delete+insert',
          unique_key=['source_system','source_record_id'],
          on_schema_change='append_new_columns',
          post_hook=drop_superseded_rows) }}

with versions as (

    select{{ payload_projection }}
    from {{ ref('stg_crm') }}

    union all

    select{{ payload_projection }}
    from {{ ref('stg_billing') }}

    union all

    select{{ payload_projection }}
    from {{ ref('stg_webforms') }}

),

current_version as (

    {#- The supersession rule itself. `qualify` rather than a join back onto a
        max(): one pass, and the tiebreak stays beside the ordering it breaks. -#}
    select{{ payload_projection }}
    from versions
    qualify row_number() over (
        partition by source_system, source_record_id
        order by ingested_at desc, ingest_batch_id desc
    ) = 1

)

select
    source_system || ':' || source_record_id as record_key,{{ payload_projection }}
from current_version
where content_hash <> '{{ tombstone_content_hash }}'
