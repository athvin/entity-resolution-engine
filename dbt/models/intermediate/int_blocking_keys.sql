{#
  S4.2 `int_blocking_keys` -- the blocking key set, macro-generated from the config.

  There is deliberately no SQL in this file. The whole body is one macro call, and
  the macro renders one `UNION ALL` branch per entry of the `blocking` var the CLI
  passes (S4.2, `dbt/macros/blocking/int_blocking_keys_union.sql`). A key expression
  written here would be a second source of truth beside `configs/*.yaml` `blocking:`,
  which is the drift M12 named: adding a `blocking:` entry must change the emitted
  key set with no model edit, and it does only while this file has nothing to edit.

  S5 gives the relation `(key_type, key_value, record_key, source_system,
  source_record_id)` and `schema.yml` holds that column list under an enforced
  contract (S5.0). `record_key` travels on every row because it is the canonical
  scalar record identity (S5.0, D6) and the pair set T-BLK-1 derives from this table
  is expressed in it; `(source_system, source_record_id)` travels because it is this
  model's incremental `unique_key`.

  **The incremental configuration is S4.2's, and the easy thing to get wrong about it
  is that it is correct.** `delete+insert` on `(source_system, source_record_id)`
  looks wrong for a relation that holds MANY rows per record -- but that is exactly
  the required behaviour: the strategy deletes every row belonging to a touched
  record before re-inserting that record's full key set, so a record whose
  standardized values changed cannot keep a key derived from the old ones. A
  `unique_key` of `(key_type, key_value, record_key)` -- the logical key -- would
  delete only the rows that came back unchanged and leave the stale ones standing.

  This relation is NOT an input to scoring (S4.3.4, D2). Splink regenerates its own
  candidate pairs from the same `blocking_rules_from_config` output; this table is
  the rule source, the S4.5 touched-subgraph driver and the `candidate_pair_count`
  benchmark metric, and nothing reads it as evidence.
#}
{{ config(materialized='incremental', incremental_strategy='delete+insert',
          unique_key=['source_system','source_record_id'],
          on_schema_change='append_new_columns') }}

{{ int_blocking_keys_union() }}
