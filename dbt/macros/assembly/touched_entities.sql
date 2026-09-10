{#
  S4.6's touched-only assembly filter, and why it is a join rather than a var payload.

  `er assemble --touched-only` rebuilds exactly the entities this run changed. The
  touched set is written to `er_touched_entities(run_id, entity_id, disposition)` by
  ER-092's `assemble.py` BEFORE the marts run, and the marts select it back through
  this macro — because passing tens of thousands of ULIDs as a `--vars` payload
  hard-fails with E2BIG (M10). No entity id ever appears on the dbt argv; the only
  per-run value is `run_id`, which every invocation already carries.

  Two dispositions, and this macro reads only one. `rebuild` entities are the ones a
  mart re-materialises; `retire` entities (a merge loser, an emptied split fragment, a
  tombstoned entity) are DELETED after the marts by the explicit reap step, because
  dbt's `delete+insert` can only delete a key present in the incoming batch and a
  retired entity is by definition absent from it. So this macro restricts the built
  set to `disposition = 'rebuild'`; the reap is the retire half and lives in Python.

  In full mode (`assemble_touched_only` unset or false) the predicate is `true`: every
  active entity with a member is rebuilt, which is the `er assemble` no-flag path and
  the shape T-MATCH-1b and T-GOLD-1 assert against.
#}

{%- macro assemble_entity_filter(entity_column='entity_id') -%}
  {%- if var('assemble_touched_only', false) -%}
    {{ entity_column }} in (
        select entity_id from {{ source('lake', 'er_touched_entities') }}
         where run_id = '{{ var('run_id') }}'
           and disposition = 'rebuild'
    )
  {%- else -%}
    true
  {%- endif -%}
{%- endmacro -%}
