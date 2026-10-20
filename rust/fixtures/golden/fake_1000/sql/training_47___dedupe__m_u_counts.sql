-- templated: __dedupe__m_u_counts
-- physical: __dedupe__m_u_counts_6a71c81ca
CREATE TABLE __dedupe__m_u_counts_6a71c81ca AS
WITH

__dedupe__match_weight_parts as (
    select
    gamma_first_name,
    CASE
        WHEN gamma_first_name = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_first_name = 4 THEN
            cast(6.626402035305282 as float8)
        WHEN gamma_first_name = 3 THEN
            cast(6.578702148649762 as float8)
        WHEN gamma_first_name = 2 THEN
            cast(6.037894127961432 as float8)
        WHEN gamma_first_name = 1 THEN
            cast(2.2078772749734696 as float8)
        WHEN gamma_first_name = 0 THEN
            cast(-4.834910971335501 as float8)
    END as mw_first_name,
    gamma_surname,
    CASE
        WHEN gamma_surname = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_surname = 4 THEN
            cast(6.908126989026417 as float8)
        WHEN gamma_surname = 3 THEN
            cast(6.81108999717025 as float8)
        WHEN gamma_surname = 2 THEN
            cast(5.209534379731118 as float8)
        WHEN gamma_surname = 1 THEN
            cast(2.563387976352057 as float8)
        WHEN gamma_surname = 0 THEN
            cast(-4.32327788293323 as float8)
    END as mw_surname,
    gamma_city,
    CASE
        WHEN gamma_city = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_city = 1 THEN
            cast(2.3447430149018658 as float8)
        WHEN gamma_city = 0 THEN
            cast(-1.0258725849816177 as float8)
    END as mw_city,
    gamma_email,
    CASE
        WHEN gamma_email = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_email = 4 THEN
            cast(8.017212882733997 as float8)
        WHEN gamma_email = 3 THEN
            cast(8.025169971390868 as float8)
        WHEN gamma_email = 2 THEN
            cast(8.089461873586377 as float8)
        WHEN gamma_email = 1 THEN
            cast(-9.254145144955872 as float8)
        WHEN gamma_email = 0 THEN
            cast(-15.24900804447567 as float8)
    END as mw_email,
    agreement_pattern_count
    from __dedupe__agreement_pattern_counts_f3ed466e6
), 

__dedupe__predict as (
    select
    cast(1.8041685038749966 as float8) + mw_first_name + mw_surname + mw_city + mw_email as match_weight,
    CASE
            WHEN (cast(1.8041685038749966 as float8) + mw_first_name + mw_surname + mw_city + mw_email) >= 0 THEN
                1.0 / (1.0 + POWER(cast(2 as float8), -(cast(1.8041685038749966 as float8) + mw_first_name + mw_surname + mw_city + mw_email)))
            ELSE
                POWER(cast(2 as float8), (cast(1.8041685038749966 as float8) + mw_first_name + mw_surname + mw_city + mw_email)) /
                (1.0 + POWER(cast(2 as float8), (cast(1.8041685038749966 as float8) + mw_first_name + mw_surname + mw_city + mw_email)))
        END as match_probability,
    gamma_first_name,
    mw_first_name,
    gamma_surname,
    mw_surname,
    gamma_city,
    mw_city,
    gamma_email,
    mw_email,
    agreement_pattern_count
    from __dedupe__match_weight_parts
)
select
gamma_first_name as comparison_vector_value,
sum(match_probability * agreement_pattern_count) as m_count,
sum((1-match_probability) * agreement_pattern_count) as u_count,
'first_name' as output_column_name
from __dedupe__predict
group by gamma_first_name

UNION ALL

select
gamma_surname as comparison_vector_value,
sum(match_probability * agreement_pattern_count) as m_count,
sum((1-match_probability) * agreement_pattern_count) as u_count,
'surname' as output_column_name
from __dedupe__predict
group by gamma_surname

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
