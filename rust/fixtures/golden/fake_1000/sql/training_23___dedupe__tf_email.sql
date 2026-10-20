-- templated: __dedupe__tf_email
-- physical: __dedupe__tf_email_631ddbc0a
CREATE TABLE __dedupe__tf_email_631ddbc0a AS
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
"email",
cast(count(*) as float8) / (
    select count("email") as total
    from __dedupe__concat
) as "tf_email"
from __dedupe__concat
where "email" is not null
group by "email"
