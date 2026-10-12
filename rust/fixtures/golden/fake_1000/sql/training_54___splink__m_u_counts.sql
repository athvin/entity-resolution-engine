-- templated: __splink__m_u_counts
-- physical: __splink__m_u_counts_1da07948b
CREATE TABLE __splink__m_u_counts_1da07948b AS
WITH

__splink__df_match_weight_parts as (
    select
    gamma_first_name,
    CASE
        WHEN gamma_first_name = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_first_name = 4 THEN
            cast(6.313435650698366 as float8)
        WHEN gamma_first_name = 3 THEN
            cast(6.312173983546117 as float8)
        WHEN gamma_first_name = 2 THEN
            cast(5.7797573733636005 as float8)
        WHEN gamma_first_name = 1 THEN
            cast(2.138815309942239 as float8)
        WHEN gamma_first_name = 0 THEN
            cast(-2.263531048594982 as float8)
    END as mw_first_name,
    gamma_surname,
    CASE
        WHEN gamma_surname = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_surname = 4 THEN
            cast(6.583764244317249 as float8)
        WHEN gamma_surname = 3 THEN
            cast(6.550307046210918 as float8)
        WHEN gamma_surname = 2 THEN
            cast(5.000727938865586 as float8)
        WHEN gamma_surname = 1 THEN
            cast(2.454690110416534 as float8)
        WHEN gamma_surname = 0 THEN
            cast(-2.175602138773552 as float8)
    END as mw_surname,
    gamma_city,
    CASE
        WHEN gamma_city = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_city = 1 THEN
            cast(2.32478303657723 as float8)
        WHEN gamma_city = 0 THEN
            cast(-1.000522027474483 as float8)
    END as mw_city,
    gamma_email,
    CASE
        WHEN gamma_email = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_email = 4 THEN
            cast(8.058917088973839 as float8)
        WHEN gamma_email = 3 THEN
            cast(7.968910949476897 as float8)
        WHEN gamma_email = 2 THEN
            cast(8.046436101193459 as float8)
        WHEN gamma_email = 1 THEN
            cast(-9.254145144955872 as float8)
        WHEN gamma_email = 0 THEN
            cast(-11.022136919863943 as float8)
    END as mw_email,
    agreement_pattern_count
    from __splink__agreement_pattern_counts_f3ed466e6
), 

__splink__df_predict as (
    select
    cast(4.336987542223659 as float8) + mw_first_name + mw_surname + mw_city + mw_email as match_weight,
    CASE
            WHEN (cast(4.336987542223659 as float8) + mw_first_name + mw_surname + mw_city + mw_email) >= 0 THEN
                1.0 / (1.0 + POWER(cast(2 as float8), -(cast(4.336987542223659 as float8) + mw_first_name + mw_surname + mw_city + mw_email)))
            ELSE
                POWER(cast(2 as float8), (cast(4.336987542223659 as float8) + mw_first_name + mw_surname + mw_city + mw_email)) /
                (1.0 + POWER(cast(2 as float8), (cast(4.336987542223659 as float8) + mw_first_name + mw_surname + mw_city + mw_email)))
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
