-- templated: __splink__m_u_counts
-- physical: __splink__m_u_counts_423fe6c05
CREATE TABLE __splink__m_u_counts_423fe6c05 AS
WITH

__splink__df_match_weight_parts as (
    select
    gamma_dob,
    CASE
        WHEN gamma_dob = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_dob = 2 THEN
            cast(8.996493872732788 as float8)
        WHEN gamma_dob = 1 THEN
            cast(4.081692660781431 as float8)
        WHEN gamma_dob = 0 THEN
            cast(-5.31710654504937 as float8)
    END as mw_dob,
    gamma_city,
    CASE
        WHEN gamma_city = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_city = 1 THEN
            cast(3.0986322543599676 as float8)
        WHEN gamma_city = 0 THEN
            cast(-4.152341215662592 as float8)
    END as mw_city,
    gamma_email,
    CASE
        WHEN gamma_email = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_email = 4 THEN
            cast(8.865181092071047 as float8)
        WHEN gamma_email = 3 THEN
            cast(3.8093520776503116 as float8)
        WHEN gamma_email = 2 THEN
            cast(3.82694613766505 as float8)
        WHEN gamma_email = 1 THEN
            cast(4.35549532948094 as float8)
        WHEN gamma_email = 0 THEN
            cast(-6.3155368376586845 as float8)
    END as mw_email,
    agreement_pattern_count
    from __splink__agreement_pattern_counts_f055dd20c
), 

__splink__df_predict as (
    select
    cast(6.5861572774898205 as float8) + mw_dob + mw_city + mw_email as match_weight,
    CASE
            WHEN (cast(6.5861572774898205 as float8) + mw_dob + mw_city + mw_email) >= 0 THEN
                1.0 / (1.0 + POWER(cast(2 as float8), -(cast(6.5861572774898205 as float8) + mw_dob + mw_city + mw_email)))
            ELSE
                POWER(cast(2 as float8), (cast(6.5861572774898205 as float8) + mw_dob + mw_city + mw_email)) /
                (1.0 + POWER(cast(2 as float8), (cast(6.5861572774898205 as float8) + mw_dob + mw_city + mw_email)))
        END as match_probability,
    gamma_dob,
    mw_dob,
    gamma_city,
    mw_city,
    gamma_email,
    mw_email,
    agreement_pattern_count
    from __splink__df_match_weight_parts
)
select
gamma_dob as comparison_vector_value,
sum(match_probability * agreement_pattern_count) as m_count,
sum((1-match_probability) * agreement_pattern_count) as u_count,
'dob' as output_column_name
from __splink__df_predict
group by gamma_dob

UNION ALL

select
gamma_city as comparison_vector_value,
sum(match_probability * agreement_pattern_count) as m_count,
sum((1-match_probability) * agreement_pattern_count) as u_count,
'city' as output_column_name
from __splink__df_predict
group by gamma_city

UNION ALL

select
gamma_email as comparison_vector_value,
sum(match_probability * agreement_pattern_count) as m_count,
sum((1-match_probability) * agreement_pattern_count) as u_count,
'email' as output_column_name
from __splink__df_predict
group by gamma_email

UNION ALL

select 0 as comparison_vector_value,
       sum(match_probability * agreement_pattern_count) /
           sum(agreement_pattern_count) as m_count,
       sum((1-match_probability) * agreement_pattern_count) /
           sum(agreement_pattern_count) as u_count,
       '_probability_two_random_records_match' as output_column_name
from __splink__df_predict
