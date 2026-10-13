-- templated: __splink__df_tf_first_name
-- physical: __splink__df_tf_first_name_4fee53224
CREATE TABLE __splink__df_tf_first_name_4fee53224 AS
WITH

__splink__df_concat as (
    select
    "unique_id",
    "first_name",
    "surname",
    "dob",
    "city",
    "email",
    "cluster"
            from fake_1000
)
select
"first_name",
cast(count(*) as float8) / (
    select count("first_name") as total
    from __splink__df_concat
) as "tf_first_name"
from __splink__df_concat
where "first_name" is not null
group by "first_name"
