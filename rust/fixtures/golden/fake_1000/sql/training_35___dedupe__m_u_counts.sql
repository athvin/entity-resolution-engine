-- templated: __dedupe__m_u_counts
-- physical: __dedupe__m_u_counts_6ca60ce01
CREATE TABLE __dedupe__m_u_counts_6ca60ce01 AS
WITH

__dedupe__match_weight_parts as (
    select
    gamma_dob,
    CASE
        WHEN gamma_dob = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_dob = 2 THEN
            cast(7.727069614376231 as float8)
        WHEN gamma_dob = 1 THEN
            cast(6.669832335010815 as float8)
        WHEN gamma_dob = 0 THEN
            cast(-1.1293854196908981 as float8)
    END as mw_dob,
    gamma_city,
    CASE
        WHEN gamma_city = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_city = 1 THEN
            cast(2.362424658319858 as float8)
        WHEN gamma_city = 0 THEN
            cast(-1.0490099583148047 as float8)
    END as mw_city,
    gamma_email,
    CASE
        WHEN gamma_email = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_email = 4 THEN
            cast(8.117442789200478 as float8)
        WHEN gamma_email = 3 THEN
            cast(7.942742920019843 as float8)
        WHEN gamma_email = 2 THEN
            cast(7.799711739299302 as float8)
        WHEN gamma_email = 1 THEN
            cast(-9.254145144955872 as float8)
        WHEN gamma_email = 0 THEN
            cast(-5.741641461530051 as float8)
    END as mw_email,
    agreement_pattern_count
    from __dedupe__agreement_pattern_counts_f055dd20c
), 

__dedupe__predict as (
    select
    cast(5.144603631802169 as float8) + mw_dob + mw_city + mw_email as match_weight,
    CASE
            WHEN (cast(5.144603631802169 as float8) + mw_dob + mw_city + mw_email) >= 0 THEN
                1.0 / (1.0 + POWER(cast(2 as float8), -(cast(5.144603631802169 as float8) + mw_dob + mw_city + mw_email)))
            ELSE
                POWER(cast(2 as float8), (cast(5.144603631802169 as float8) + mw_dob + mw_city + mw_email)) /
                (1.0 + POWER(cast(2 as float8), (cast(5.144603631802169 as float8) + mw_dob + mw_city + mw_email)))
        END as match_probability,
    gamma_dob,
    mw_dob,
    gamma_city,
    mw_city,
    gamma_email,
    mw_email,
    agreement_pattern_count
    from __dedupe__match_weight_parts
)
select
gamma_dob as comparison_vector_value,
sum(match_probability * agreement_pattern_count) as m_count,
sum((1-match_probability) * agreement_pattern_count) as u_count,
'dob' as output_column_name
from __dedupe__predict
group by gamma_dob

UNION ALL

select
gamma_city as comparison_vector_value,
sum(match_probability * agreement_pattern_count) as m_count,
sum((1-match_probability) * agreement_pattern_count) as u_count,
'city' as output_column_name
from __dedupe__predict
group by gamma_city

UNION ALL

select
gamma_email as comparison_vector_value,
sum(match_probability * agreement_pattern_count) as m_count,
sum((1-match_probability) * agreement_pattern_count) as u_count,
'email' as output_column_name
from __dedupe__predict
group by gamma_email

UNION ALL

select 0 as comparison_vector_value,
       sum(match_probability * agreement_pattern_count) /
           sum(agreement_pattern_count) as m_count,
       sum((1-match_probability) * agreement_pattern_count) /
           sum(agreement_pattern_count) as u_count,
       '_probability_two_random_records_match' as output_column_name
from __dedupe__predict
