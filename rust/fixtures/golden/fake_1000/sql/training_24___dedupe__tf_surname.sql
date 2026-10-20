-- templated: __dedupe__tf_surname
-- physical: __dedupe__tf_surname_41b4a4bab
CREATE TABLE __dedupe__tf_surname_41b4a4bab AS
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
"surname",
cast(count(*) as float8) / (
    select count("surname") as total
    from __dedupe__concat
) as "tf_surname"
from __dedupe__concat
where "surname" is not null
group by "surname"
