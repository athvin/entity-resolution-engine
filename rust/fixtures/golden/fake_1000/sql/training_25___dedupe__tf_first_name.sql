-- templated: __dedupe__tf_first_name
-- physical: __dedupe__tf_first_name_4fee53224
CREATE TABLE __dedupe__tf_first_name_4fee53224 AS
WITH

__dedupe__concat as (
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
    from __dedupe__concat
) as "tf_first_name"
from __dedupe__concat
where "first_name" is not null
group by "first_name"
