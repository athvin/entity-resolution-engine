-- templated: __dedupe__concat_count
-- physical: __dedupe__concat_count_f8cae82af
CREATE TABLE __dedupe__concat_count_f8cae82af AS
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
