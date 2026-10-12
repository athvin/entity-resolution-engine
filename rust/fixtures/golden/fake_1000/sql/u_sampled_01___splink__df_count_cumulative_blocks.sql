-- templated: __splink__df_count_cumulative_blocks
-- physical: __splink__df_count_cumulative_blocks_5cb2b47ba
CREATE TABLE __splink__df_count_cumulative_blocks_5cb2b47ba AS
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
), 

__splink__blocked_id_pairs as (
    select
    '0' as match_key,
    l."unique_id" as join_key_l,
    r."unique_id" as join_key_r
    from __splink__df_concat as l
    inner join __splink__df_concat as r
    on
    ((l."first_name" = r."first_name") AND (l."dob" = r."dob"))
    where l."unique_id" < r."unique_id"

    UNION ALL

    select
    '1' as match_key,
    l."unique_id" as join_key_l,
    r."unique_id" as join_key_r
    from __splink__df_concat as l
    inner join __splink__df_concat as r
    on
    (l.first_name = r.first_name and levenshtein(r.surname, l.surname) <= 2)
    where l."unique_id" < r."unique_id"
    AND NOT (coalesce(((l."first_name" = r."first_name") AND (l."dob" = r."dob")),false))

    UNION ALL

    select
    '2' as match_key,
    l."unique_id" as join_key_l,
    r."unique_id" as join_key_r
    from __splink__df_concat as l
    inner join __splink__df_concat as r
    on
    (l."email" = r."email")
    where l."unique_id" < r."unique_id"
    AND NOT (coalesce(((l."first_name" = r."first_name") AND (l."dob" = r."dob")),false) OR coalesce((l.first_name = r.first_name and levenshtein(r.surname, l.surname) <= 2),false))
)
select
count(*) as row_count,
match_key
from __splink__blocked_id_pairs
group by match_key
order by cast(match_key as int) asc
