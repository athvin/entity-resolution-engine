-- templated: __dedupe__m_u_counts
-- physical: __dedupe__m_u_counts_2c75bdb0c
CREATE TABLE __dedupe__m_u_counts_2c75bdb0c AS
WITH

__dedupe__match_weight_parts as (
    select
    gamma_dob,
    CASE
        WHEN gamma_dob = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_dob = 2 THEN
            cast(7.848172413146346 as float8)
        WHEN gamma_dob = 1 THEN
            cast(6.787415123322939 as float8)
        WHEN gamma_dob = 0 THEN
            cast(-1.2874361904571718 as float8)
    END as mw_dob,
    gamma_city,
    CASE
        WHEN gamma_city = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_city = 1 THEN
            cast(2.512014783214708 as float8)
        WHEN gamma_city = 0 THEN
            cast(-1.275017976491643 as float8)
    END as mw_city,
    gamma_email,
    CASE
        WHEN gamma_email = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_email = 4 THEN
            cast(8.165480543042143 as float8)
        WHEN gamma_email = 3 THEN
            cast(7.944551615163308 as float8)
        WHEN gamma_email = 2 THEN
            cast(7.694256201352146 as float8)
        WHEN gamma_email = 1 THEN
            cast(-9.254145144955872 as float8)
        WHEN gamma_email = 0 THEN
            cast(-6.257818095458805 as float8)
    END as mw_email,
    agreement_pattern_count
    from __dedupe__agreement_pattern_counts_f055dd20c
), 

__dedupe__predict as (
    select
    cast(3.079473428063803 as float8) + mw_dob + mw_city + mw_email as match_weight,
    CASE
            WHEN (cast(3.079473428063803 as float8) + mw_dob + mw_city + mw_email) >= 0 THEN
                1.0 / (1.0 + POWER(cast(2 as float8), -(cast(3.079473428063803 as float8) + mw_dob + mw_city + mw_email)))
            ELSE
                POWER(cast(2 as float8), (cast(3.079473428063803 as float8) + mw_dob + mw_city + mw_email)) /
                (1.0 + POWER(cast(2 as float8), (cast(3.079473428063803 as float8) + mw_dob + mw_city + mw_email)))
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
