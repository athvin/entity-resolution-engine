{{ config(tags=['keys']) }}

-- S5.0 / D6: `record_key` is `source_system || ':' || source_record_id`, and
-- `source_record_id` MUST NOT contain `':'` -- enforced by a dbt singular test on
-- `int_std_records`, which is this file.
--
-- The ban is not tidiness. `record_key` is a scalar identity that has to be
-- *splittable* back into its two parts (`er.entities.ids.split_record_key`), and a
-- `':'` inside `source_record_id` makes the split ambiguous: `crm:a:1` is both
-- `('crm', 'a:1')` and `('crm:a', '1')`. Every relation that stores a `record_key`
-- -- `int_blocking_keys`, `entity_membership`, and every pair relation of S5.0 --
-- inherits the ambiguity from here, which is why the test guards the relation the
-- key is first materialized on. S4.7 classifies a `source_record_id` containing
-- `':'` as a `data` error, so this failing IS the reported defect.
--
-- Both arms in one test, because they are one claim. A colon in the id is how the
-- identity becomes ambiguous; a `record_key` that is not the concatenation is how
-- it becomes wrong outright, and a reader who sees one failing wants to know which.

select
    record_key,
    source_system,
    source_record_id,
    case
        when source_record_id like '%:%' then 'colon_in_source_record_id'
        else 'record_key_is_not_the_concatenation'
    end as violation
from {{ ref('int_std_records') }}
where source_record_id like '%:%'
   or record_key is distinct from source_system || ':' || source_record_id
