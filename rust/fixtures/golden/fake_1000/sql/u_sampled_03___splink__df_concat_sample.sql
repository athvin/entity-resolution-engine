-- templated: __splink__df_concat_sample
-- physical: __splink__df_concat_sample_54056a710
CREATE TABLE __splink__df_concat_sample_54056a710 AS
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
select *
from __splink__df_concat
where 1=1
 AND hash("unique_id") % 1000000000 < 632955730
