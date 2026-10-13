-- templated: __splink__m_u_counts
-- physical: __splink__m_u_counts_7a7918230
CREATE TABLE __splink__m_u_counts_7a7918230 AS
WITH

__splink__blocked_id_pairs as (
    select
    l."unique_id" as join_key_l,
    r."unique_id" as join_key_r,
    "l"."unique_id" AS "unique_id_l",
    "r"."unique_id" AS "unique_id_r",
    "l"."first_name" AS "first_name_l",
    "r"."first_name" AS "first_name_r",
    '0' as match_key
    from __splink__df_concat_sample_54056a710 as l
    inner join __splink__df_concat_sample_54056a710 as r
    on
    (1=1)
    where l."unique_id" < r."unique_id" AND (ABS(hash(r."unique_id")) % 10) + 1 = 1
), 

__splink__df_comparison_vectors as (
    select
    "unique_id_l",
    "unique_id_r",
    CASE
        WHEN "first_name_l" IS NULL OR "first_name_r" IS NULL THEN -1
        WHEN "first_name_l" = "first_name_r" THEN 4
        WHEN jaro_winkler_similarity("first_name_l", "first_name_r") >= 0.92 THEN 3
        WHEN jaro_winkler_similarity("first_name_l", "first_name_r") >= 0.88 THEN 2
        WHEN jaro_winkler_similarity("first_name_l", "first_name_r") >= 0.7 THEN 1
        ELSE 0
    END as gamma_first_name,
    match_key
    from __splink__blocked_id_pairs
), 

__splink__df_predict as (
    select *, cast(0.0 as float8) as match_probability
    from __splink__df_comparison_vectors
)
select
gamma_first_name as comparison_vector_value,
sum(match_probability * 1) as m_count,
sum((1-match_probability) * 1) as u_count,
'first_name' as output_column_name
from __splink__df_predict
group by gamma_first_name

UNION ALL

select 0 as comparison_vector_value,
       sum(match_probability * 1) /
           sum(1) as m_count,
       sum((1-match_probability) * 1) /
           sum(1) as u_count,
       '_probability_two_random_records_match' as output_column_name
from __splink__df_predict
