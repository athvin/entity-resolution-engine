-- templated: __splink__blocked_id_pairs
-- physical: __splink__blocked_id_pairs_71d3c1477
CREATE TABLE __splink__blocked_id_pairs_71d3c1477 AS
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
'0' as match_key,
l."unique_id" as join_key_l,
r."unique_id" as join_key_r
from __splink__df_concat as l
inner join __splink__df_concat as r
on
((l."first_name" = r."first_name") AND (l."surname" = r."surname"))
where l."unique_id" < r."unique_id"
