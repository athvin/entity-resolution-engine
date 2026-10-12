-- templated: __splink__m_u_counts
-- physical: __splink__m_u_counts_6cf7ebb76
CREATE TABLE __splink__m_u_counts_6cf7ebb76 AS
WITH

__splink__blocked_id_pairs as (
    select
    l."unique_id" as join_key_l,
    r."unique_id" as join_key_r,
    "l"."unique_id" AS "unique_id_l",
    "r"."unique_id" AS "unique_id_r",
    "l"."email" AS "email_l",
    "r"."email" AS "email_r",
    '0' as match_key
    from __splink__df_concat_sample_a5239093d as l
    inner join __splink__df_concat_sample_a5239093d as r
    on
    (1=1)
    where l."unique_id" < r."unique_id" AND (ABS(hash(r."unique_id")) % 10) + 1 = 4
), 

__splink__df_comparison_vectors as (
    select
    "unique_id_l",
    "unique_id_r",
    CASE
        WHEN "email_l" IS NULL OR "email_r" IS NULL THEN -1
        WHEN "email_l" = "email_r" THEN 4
        WHEN NULLIF(regexp_extract("email_l", '^[^@]+', 0), '') = NULLIF(regexp_extract("email_r", '^[^@]+', 0), '') THEN 3
        WHEN jaro_winkler_similarity("email_l", "email_r") >= 0.88 THEN 2
        WHEN jaro_winkler_similarity(NULLIF(regexp_extract("email_l", '^[^@]+', 0), ''), NULLIF(regexp_extract("email_r", '^[^@]+', 0), '')) >= 0.88 THEN 1
        ELSE 0
    END as gamma_email,
    match_key
    from __splink__blocked_id_pairs
), 

__splink__df_predict as (
    select *, cast(0.0 as float8) as match_probability
    from __splink__df_comparison_vectors
)
select
gamma_email as comparison_vector_value,
sum(match_probability * 1) as m_count,
sum((1-match_probability) * 1) as u_count,
'email' as output_column_name
from __splink__df_predict
group by gamma_email

UNION ALL

select 0 as comparison_vector_value,
       sum(match_probability * 1) /
           sum(1) as m_count,
       sum((1-match_probability) * 1) /
           sum(1) as u_count,
       '_probability_two_random_records_match' as output_column_name
from __splink__df_predict
