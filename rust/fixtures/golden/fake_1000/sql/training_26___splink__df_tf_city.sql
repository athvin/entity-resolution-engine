-- templated: __splink__df_tf_city
-- physical: __splink__df_tf_city_b487f6ea2
CREATE TABLE __splink__df_tf_city_b487f6ea2 AS
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
"city",
cast(count(*) as float8) / (
    select count("city") as total
    from __splink__df_concat
) as "tf_city"
from __splink__df_concat
where "city" is not null
group by "city"
