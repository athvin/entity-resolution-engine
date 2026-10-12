-- templated: __splink__m_u_counts
-- physical: __splink__m_u_counts_c463a6dd6
CREATE TABLE __splink__m_u_counts_c463a6dd6 AS
WITH

__splink__df_match_weight_parts as (
    select
    gamma_first_name,
    CASE
        WHEN gamma_first_name = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_first_name = 4 THEN
            cast(6.318843762135857 as float8)
        WHEN gamma_first_name = 3 THEN
            cast(6.317580873456069 as float8)
        WHEN gamma_first_name = 2 THEN
            cast(5.785146902918606 as float8)
        WHEN gamma_first_name = 1 THEN
            cast(2.1432818372810156 as float8)
        WHEN gamma_first_name = 0 THEN
            cast(-2.2846397480021534 as float8)
    END as mw_first_name,
    gamma_surname,
    CASE
        WHEN gamma_surname = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_surname = 4 THEN
            cast(6.5890177014140505 as float8)
        WHEN gamma_surname = 3 THEN
            cast(6.55305676329413 as float8)
        WHEN gamma_surname = 2 THEN
            cast(5.005864039358421 as float8)
        WHEN gamma_surname = 1 THEN
            cast(2.4595208616762543 as float8)
        WHEN gamma_surname = 0 THEN
            cast(-2.191956959515921 as float8)
    END as mw_surname,
    gamma_city,
    CASE
        WHEN gamma_city = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_city = 1 THEN
            cast(2.3260845866496376 as float8)
        WHEN gamma_city = 0 THEN
            cast(-1.0021509919617526 as float8)
    END as mw_city,
    gamma_email,
    CASE
        WHEN gamma_email = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_email = 4 THEN
            cast(8.059514441674951 as float8)
        WHEN gamma_email = 3 THEN
            cast(7.969538561951855 as float8)
        WHEN gamma_email = 2 THEN
            cast(8.047075579850919 as float8)
        WHEN gamma_email = 1 THEN
            cast(-9.254145144955872 as float8)
        WHEN gamma_email = 0 THEN
            cast(-14.189384562618196 as float8)
    END as mw_email,
    agreement_pattern_count
    from __splink__agreement_pattern_counts_f3ed466e6
), 

__splink__df_predict as (
    select
    cast(4.2437742907346205 as float8) + mw_first_name + mw_surname + mw_city + mw_email as match_weight,
    CASE
            WHEN (cast(4.2437742907346205 as float8) + mw_first_name + mw_surname + mw_city + mw_email) >= 0 THEN
                1.0 / (1.0 + POWER(cast(2 as float8), -(cast(4.2437742907346205 as float8) + mw_first_name + mw_surname + mw_city + mw_email)))
            ELSE
                POWER(cast(2 as float8), (cast(4.2437742907346205 as float8) + mw_first_name + mw_surname + mw_city + mw_email)) /
                (1.0 + POWER(cast(2 as float8), (cast(4.2437742907346205 as float8) + mw_first_name + mw_surname + mw_city + mw_email)))
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
    from __splink__df_match_weight_parts
)
select
gamma_first_name as comparison_vector_value,
sum(match_probability * agreement_pattern_count) as m_count,
sum((1-match_probability) * agreement_pattern_count) as u_count,
'first_name' as output_column_name
from __splink__df_predict
group by gamma_first_name

UNION ALL

select
gamma_surname as comparison_vector_value,
sum(match_probability * agreement_pattern_count) as m_count,
sum((1-match_probability) * agreement_pattern_count) as u_count,
'surname' as output_column_name
from __splink__df_predict
group by gamma_surname

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
