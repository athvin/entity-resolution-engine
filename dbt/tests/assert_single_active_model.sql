{{ config(tags=['keys']) }}

-- S5.0's second `model_registry` key: at most one row with `status='active'`.
--
-- It is a singular test because it is uniqueness over *no* columns under a filter,
-- which no generic test can express: `unique` needs a column and
-- `dbt_utils.unique_combination_of_columns` needs a non-empty combination. The
-- registry spells it as a `LogicalKey((), where="status='active'")`, and
-- `er.lake.dbt_sources` skips exactly that shape when it renders `sources.yml`.
--
-- What it protects is S4.3.2's model activation: `er match` resolves the active
-- model by this predicate, so two active rows would make the `model_version` every
-- score is stamped with depend on row order.

select count(*) as active_models
from {{ source('lake', 'model_registry') }}
where status = 'active'
having count(*) > 1
