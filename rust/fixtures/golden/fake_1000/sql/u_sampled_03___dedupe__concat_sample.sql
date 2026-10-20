-- templated: __dedupe__concat_sample
-- physical: __dedupe__concat_sample_54056a710
CREATE TABLE __dedupe__concat_sample_54056a710 AS
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
 AND hash("unique_id") % 1000000000 < 632955730
