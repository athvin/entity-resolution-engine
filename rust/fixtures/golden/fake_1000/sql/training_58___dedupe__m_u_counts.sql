-- templated: __dedupe__m_u_counts
-- physical: __dedupe__m_u_counts_916068508
CREATE TABLE __dedupe__m_u_counts_916068508 AS
WITH

__dedupe__match_weight_parts as (
    select
    gamma_first_name,
    CASE
        WHEN gamma_first_name = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_first_name = 4 THEN
            cast(6.309469306422203 as float8)
        WHEN gamma_first_name = 3 THEN
            cast(6.3082083437620105 as float8)
        WHEN gamma_first_name = 2 THEN
            cast(5.775800929783022 as float8)
        WHEN gamma_first_name = 1 THEN
            cast(2.1353151776022092 as float8)
        WHEN gamma_first_name = 0 THEN
            cast(-2.248204289503825 as float8)
    END as mw_first_name,
    gamma_surname,
    CASE
        WHEN gamma_surname = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_surname = 4 THEN
            cast(6.579917547884861 as float8)
        WHEN gamma_surname = 3 THEN
            cast(6.552151979924186 as float8)
        WHEN gamma_surname = 2 THEN
            cast(4.996943132746048 as float8)
        WHEN gamma_surname = 1 THEN
            cast(2.4510650770410902 as float8)
        WHEN gamma_surname = 0 THEN
            cast(-2.1677443873666125 as float8)
    END as mw_surname,
    gamma_city,
    CASE
        WHEN gamma_city = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_city = 1 THEN
            cast(2.324775631335238 as float8)
        WHEN gamma_city = 0 THEN
            cast(-1.0005127688513933 as float8)
    END as mw_city,
    gamma_email,
    CASE
        WHEN gamma_email = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_email = 4 THEN
            cast(8.057455743442963 as float8)
        WHEN gamma_email = 3 THEN
            cast(7.967435282520537 as float8)
        WHEN gamma_email = 2 THEN
            cast(8.04495486691204 as float8)
        WHEN gamma_email = 1 THEN
            cast(-9.254145144955872 as float8)
        WHEN gamma_email = 0 THEN
            cast(-9.378212465469177 as float8)
    END as mw_email,
    agreement_pattern_count
    from __dedupe__agreement_pattern_counts_f3ed466e6
), 

__dedupe__predict as (
    select
    cast(4.405563178778948 as float8) + mw_first_name + mw_surname + mw_city + mw_email as match_weight,
    CASE
            WHEN (cast(4.405563178778948 as float8) + mw_first_name + mw_surname + mw_city + mw_email) >= 0 THEN
                1.0 / (1.0 + POWER(cast(2 as float8), -(cast(4.405563178778948 as float8) + mw_first_name + mw_surname + mw_city + mw_email)))
            ELSE
                POWER(cast(2 as float8), (cast(4.405563178778948 as float8) + mw_first_name + mw_surname + mw_city + mw_email)) /
                (1.0 + POWER(cast(2 as float8), (cast(4.405563178778948 as float8) + mw_first_name + mw_surname + mw_city + mw_email)))
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
