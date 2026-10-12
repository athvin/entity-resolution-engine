-- templated: __splink__blocked_id_pairs
-- physical: __splink__blocked_id_pairs_e105ce473
CREATE TABLE __splink__blocked_id_pairs_e105ce473 AS
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
(l."dob" = r."dob")
where l."unique_id" < r."unique_id"
