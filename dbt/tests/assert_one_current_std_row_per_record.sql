{{ config(tags=['keys']) }}

-- S4.2's supersession rule, in full: `int_std_records` holds exactly ONE current
-- row per `(source_system, source_record_id)`, and that row is the one derived from
-- the `raw_records` version with the greatest `ingested_at` (ties broken by
-- `ingest_batch_id DESC`).
--
-- Two violations, and the second is the reason this test is not a duplicate of the
-- `unique_combination_of_columns` in `models/intermediate/schema.yml`. That generic
-- test asserts the *cardinality* -- one row per key -- and is satisfied by a
-- relation holding one row per key that is the WRONG version. `superseded` asserts
-- the other half: no retained row may be older than a version that is staged and
-- available. A record whose current `raw_records` version is a tombstone shows up
-- here rather than anywhere else, because the tombstone is a staged version that
-- supersedes whatever content row is still standing in for the key -- which makes
-- this the test that fails if `int_std_records`' tombstone exclusion ever stops
-- being applied on an incremental run.
--
-- The version set is read from `raw_records` and NOT from the three `stg_<source>`
-- models, for two reasons that happen to agree. S4.2 states the rule against
-- `raw_records` -- the current row is "derived from the `raw_records` version with
-- the greatest `ingested_at`" -- so this is the relation the claim is about. And a
-- test whose parents are the three staging models is, under dbt's eager indirect
-- selection, dragged into every `--select staging` invocation, where
-- `int_std_records` may not have been built at all; reading the version history
-- instead leaves this test selected by `intermediate` and by `tag:keys`, which are
-- the two selections that mean to run it.

with raw_version as (

    select source_system, source_record_id, ingested_at, ingest_batch_id
    from {{ source('lake', 'raw_records') }}

),

duplicated as (

    select
        source_system,
        source_record_id,
        'more_than_one_current_row' as violation
    from {{ ref('int_std_records') }}
    group by source_system, source_record_id
    having count(*) > 1

),

superseded as (

    -- Written as two comparisons rather than a row constructor: the tiebreak is
    -- `ingest_batch_id DESC` and only within an `ingested_at` tie, which is exactly
    -- what these two lines say and what a lexicographic pair comparison would leave
    -- to the reader to infer.
    select distinct
        current_row.source_system,
        current_row.source_record_id,
        'superseded_version_retained' as violation
    from {{ ref('int_std_records') }} as current_row
    join raw_version as version
      on version.source_system = current_row.source_system
     and version.source_record_id = current_row.source_record_id
    where version.ingested_at > current_row.ingested_at
       or (version.ingested_at = current_row.ingested_at
           and version.ingest_batch_id > current_row.ingest_batch_id)

)

select source_system, source_record_id, violation from duplicated

union all

select source_system, source_record_id, violation from superseded
