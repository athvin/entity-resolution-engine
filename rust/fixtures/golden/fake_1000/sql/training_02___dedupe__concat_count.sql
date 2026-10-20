-- templated: __dedupe__concat_count
-- physical: __dedupe__concat_count_23697fafe
CREATE TABLE __dedupe__concat_count_23697fafe AS
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
select count(*) as count
from __dedupe__concat
