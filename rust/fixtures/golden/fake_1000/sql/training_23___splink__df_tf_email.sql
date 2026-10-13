-- templated: __splink__df_tf_email
-- physical: __splink__df_tf_email_631ddbc0a
CREATE TABLE __splink__df_tf_email_631ddbc0a AS
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
"email",
cast(count(*) as float8) / (
    select count("email") as total
    from __splink__df_concat
) as "tf_email"
from __splink__df_concat
where "email" is not null
group by "email"
