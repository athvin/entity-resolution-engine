-- templated: __dedupe__concat_sample
-- physical: __dedupe__concat_sample_a5239093d
CREATE TABLE __dedupe__concat_sample_a5239093d AS
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
select *
from __dedupe__concat
where 1=1
