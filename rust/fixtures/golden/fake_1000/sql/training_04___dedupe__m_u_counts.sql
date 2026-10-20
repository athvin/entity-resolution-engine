-- templated: __dedupe__m_u_counts
-- physical: __dedupe__m_u_counts_56a8aa704
CREATE TABLE __dedupe__m_u_counts_56a8aa704 AS
WITH

__dedupe__blocked_id_pairs as (
    select
    l."unique_id" as join_key_l,
    r."unique_id" as join_key_r,
    "l"."unique_id" AS "unique_id_l",
    "r"."unique_id" AS "unique_id_r",
    "l"."first_name" AS "first_name_l",
    "r"."first_name" AS "first_name_r",
    '0' as match_key
    from __dedupe__concat_sample_a5239093d as l
    inner join __dedupe__concat_sample_a5239093d as r
    on
    (1=1)
    where l."unique_id" < r."unique_id" AND (ABS(hash(r."unique_id")) % 100) + 1 = 1
), 

__dedupe__comparison_vectors as (
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
    from __dedupe__blocked_id_pairs
), 

__dedupe__predict as (
    select *, cast(0.0 as float8) as match_probability
    from __dedupe__comparison_vectors
)
select
gamma_first_name as comparison_vector_value,
sum(match_probability * 1) as m_count,
sum((1-match_probability) * 1) as u_count,
'first_name' as output_column_name
from __dedupe__predict
group by gamma_first_name

UNION ALL

select 0 as comparison_vector_value,
       sum(match_probability * 1) /
           sum(1) as m_count,
       sum((1-match_probability) * 1) /
           sum(1) as u_count,
       '_probability_two_random_records_match' as output_column_name
from __dedupe__predict
