-- templated: __dedupe__tf_city
-- physical: __dedupe__tf_city_b487f6ea2
CREATE TABLE __dedupe__tf_city_b487f6ea2 AS
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
"city",
cast(count(*) as float8) / (
    select count("city") as total
    from __dedupe__concat
) as "tf_city"
from __dedupe__concat
where "city" is not null
group by "city"
