-- templated: __splink__df_tf_surname
-- physical: __splink__df_tf_surname_41b4a4bab
CREATE TABLE __splink__df_tf_surname_41b4a4bab AS
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
"surname",
cast(count(*) as float8) / (
    select count("surname") as total
    from __splink__df_concat
) as "tf_surname"
from __splink__df_concat
where "surname" is not null
group by "surname"
