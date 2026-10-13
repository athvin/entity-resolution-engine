-- templated: __splink__m_u_counts
-- physical: __splink__m_u_counts_31f5b45cd
CREATE TABLE __splink__m_u_counts_31f5b45cd AS
WITH

__splink__df_match_weight_parts as (
    select
    gamma_first_name,
    CASE
        WHEN gamma_first_name = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_first_name = 4 THEN
            cast(6.309890288347978 as float8)
        WHEN gamma_first_name = 3 THEN
            cast(6.3086292305337714 as float8)
        WHEN gamma_first_name = 2 THEN
            cast(5.776220582440064 as float8)
        WHEN gamma_first_name = 1 THEN
            cast(2.1356732019058153 as float8)
        WHEN gamma_first_name = 0 THEN
            cast(-2.2498160547818995 as float8)
    END as mw_first_name,
    gamma_surname,
    CASE
        WHEN gamma_surname = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_surname = 4 THEN
            cast(6.580325308944047 as float8)
        WHEN gamma_surname = 3 THEN
            cast(6.552295878534566 as float8)
        WHEN gamma_surname = 2 THEN
            cast(4.997342582851463 as float8)
        WHEN gamma_surname = 1 THEN
            cast(2.451443011774606 as float8)
        WHEN gamma_surname = 0 THEN
            cast(-2.168924798186399 as float8)
    END as mw_surname,
    gamma_city,
    CASE
        WHEN gamma_city = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_city = 1 THEN
            cast(2.3248722149397736 as float8)
        WHEN gamma_city = 0 THEN
            cast(-1.0006335337622063 as float8)
    END as mw_city,
    gamma_email,
    CASE
        WHEN gamma_email = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_email = 4 THEN
            cast(8.057530623825206 as float8)
        WHEN gamma_email = 3 THEN
            cast(7.967512035678507 as float8)
        WHEN gamma_email = 2 THEN
            cast(8.045032353408175 as float8)
        WHEN gamma_email = 1 THEN
            cast(-9.254145144955872 as float8)
        WHEN gamma_email = 0 THEN
            cast(-9.429785715015411 as float8)
    END as mw_email,
    agreement_pattern_count
    from __splink__agreement_pattern_counts_f3ed466e6
), 

__splink__df_predict as (
    select
    cast(4.397808095701695 as float8) + mw_first_name + mw_surname + mw_city + mw_email as match_weight,
    CASE
            WHEN (cast(4.397808095701695 as float8) + mw_first_name + mw_surname + mw_city + mw_email) >= 0 THEN
                1.0 / (1.0 + POWER(cast(2 as float8), -(cast(4.397808095701695 as float8) + mw_first_name + mw_surname + mw_city + mw_email)))
            ELSE
                POWER(cast(2 as float8), (cast(4.397808095701695 as float8) + mw_first_name + mw_surname + mw_city + mw_email)) /
                (1.0 + POWER(cast(2 as float8), (cast(4.397808095701695 as float8) + mw_first_name + mw_surname + mw_city + mw_email)))
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
