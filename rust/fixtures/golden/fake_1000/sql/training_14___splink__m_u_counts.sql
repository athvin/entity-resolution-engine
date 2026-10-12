-- templated: __splink__m_u_counts
-- physical: __splink__m_u_counts_58b019e8e
CREATE TABLE __splink__m_u_counts_58b019e8e AS
WITH

__splink__blocked_id_pairs as (
    select
    l."unique_id" as join_key_l,
    r."unique_id" as join_key_r,
    "l"."unique_id" AS "unique_id_l",
    "r"."unique_id" AS "unique_id_r",
    "l"."dob" AS "dob_l",
    "r"."dob" AS "dob_r",
    '0' as match_key
    from __splink__df_concat_sample_a5239093d as l
    inner join __splink__df_concat_sample_a5239093d as r
    on
    (1=1)
    where l."unique_id" < r."unique_id" AND (ABS(hash(r."unique_id")) % 10) + 1 = 2
), 

__splink__df_comparison_vectors as (
    select
    "unique_id_l",
    "unique_id_r",
    CASE
        WHEN "dob_l" IS NULL OR "dob_r" IS NULL THEN -1
        WHEN "dob_l" = "dob_r" THEN 2
        WHEN levenshtein("dob_l", "dob_r") <= 1 THEN 1
        ELSE 0
    END as gamma_dob,
    match_key
    from __splink__blocked_id_pairs
), 

__splink__df_predict as (
    select *, cast(0.0 as float8) as match_probability
    from __splink__df_comparison_vectors
)
select
gamma_dob as comparison_vector_value,
sum(match_probability * 1) as m_count,
sum((1-match_probability) * 1) as u_count,
'dob' as output_column_name
from __splink__df_predict
group by gamma_dob

UNION ALL

select 0 as comparison_vector_value,
       sum(match_probability * 1) /
           sum(1) as m_count,
       sum((1-match_probability) * 1) /
           sum(1) as u_count,
       '_probability_two_random_records_match' as output_column_name
from __splink__df_predict
