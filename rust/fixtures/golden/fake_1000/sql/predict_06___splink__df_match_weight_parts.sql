-- templated: __splink__df_match_weight_parts
-- physical: __splink__df_match_weight_parts_6a69ce520
CREATE TABLE __splink__df_match_weight_parts_6a69ce520 AS

    select
    "unique_id_l",
    "unique_id_r",
    "first_name_l",
    "first_name_r",
    gamma_first_name,
    "tf_first_name_l",
    "tf_first_name_r",
    CASE
        WHEN gamma_first_name = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_first_name = 4 THEN
            cast(6.309110459952498 as float8)
        WHEN gamma_first_name = 3 THEN
            cast(6.307849583185448 as float8)
        WHEN gamma_first_name = 2 THEN
            cast(5.775443297784355 as float8)
        WHEN gamma_first_name = 1 THEN
            cast(2.1350152544819414 as float8)
        WHEN gamma_first_name = 0 THEN
            cast(-2.246834260981902 as float8)
    END as mw_first_name,
    CASE
        WHEN gamma_first_name = -1 THEN cast(0 as float8)
        WHEN gamma_first_name = 4 THEN
            CASE
                WHEN coalesce("tf_first_name_l", "tf_first_name_r") is not null THEN
                    cast(1.0 as float8) * (
                        cast(-7.336383273282888 as float8) - log2(greatest(coalesce("tf_first_name_l", "tf_first_name_r"), coalesce("tf_first_name_r", "tf_first_name_l"), cast(0.0 as float8)))
                    )
                ELSE cast(0 as float8)
            END
        WHEN gamma_first_name = 3 THEN cast(0 as float8)
        WHEN gamma_first_name = 2 THEN cast(0 as float8)
        WHEN gamma_first_name = 1 THEN cast(0 as float8)
        WHEN gamma_first_name = 0 THEN cast(0 as float8)
    END as mw_tf_adj_first_name,
    "surname_l",
    "surname_r",
    gamma_surname,
    "tf_surname_l",
    "tf_surname_r",
    CASE
        WHEN gamma_surname = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_surname = 4 THEN
            cast(6.579569246965066 as float8)
        WHEN gamma_surname = 3 THEN
            cast(6.551923052301715 as float8)
        WHEN gamma_surname = 2 THEN
            cast(4.996602442325117 as float8)
        WHEN gamma_surname = 1 THEN
            cast(2.4507441783057566 as float8)
        WHEN gamma_surname = 0 THEN
            cast(-2.166627690775239 as float8)
    END as mw_surname,
    CASE
        WHEN gamma_surname = -1 THEN cast(0 as float8)
        WHEN gamma_surname = 4 THEN
            CASE
                WHEN coalesce("tf_surname_l", "tf_surname_r") is not null THEN
                    cast(1.0 as float8) * (
                        cast(-7.783881756656416 as float8) - log2(greatest(coalesce("tf_surname_l", "tf_surname_r"), coalesce("tf_surname_r", "tf_surname_l"), cast(0.0 as float8)))
                    )
                ELSE cast(0 as float8)
            END
        WHEN gamma_surname = 3 THEN cast(0 as float8)
        WHEN gamma_surname = 2 THEN cast(0 as float8)
        WHEN gamma_surname = 1 THEN cast(0 as float8)
        WHEN gamma_surname = 0 THEN cast(0 as float8)
    END as mw_tf_adj_surname,
    "dob_l",
    "dob_r",
    gamma_dob,
    CASE
        WHEN gamma_dob = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_dob = 2 THEN
            cast(7.720033216531271 as float8)
        WHEN gamma_dob = 1 THEN
            cast(6.6633420243978545 as float8)
        WHEN gamma_dob = 0 THEN
            cast(-1.1212001603519732 as float8)
    END as mw_dob,
    "city_l",
    "city_r",
    gamma_city,
    "tf_city_l",
    "tf_city_r",
    CASE
        WHEN gamma_city = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_city = 1 THEN
            cast(2.341128076900223 as float8)
        WHEN gamma_city = 0 THEN
            cast(-1.0212220149992821 as float8)
    END as mw_city,
    CASE
        WHEN gamma_city = -1 THEN cast(0 as float8)
        WHEN gamma_city = 1 THEN
            CASE
                WHEN coalesce("tf_city_l", "tf_city_r") is not null THEN
                    cast(1.0 as float8) * (
                        cast(-3.1726328358037446 as float8) - log2(greatest(coalesce("tf_city_l", "tf_city_r"), coalesce("tf_city_r", "tf_city_l"), cast(0.0 as float8)))
                    )
                ELSE cast(0 as float8)
            END
        WHEN gamma_city = 0 THEN cast(0 as float8)
    END as mw_tf_adj_city,
    "email_l",
    "email_r",
    gamma_email,
    "tf_email_l",
    "tf_email_r",
    CASE
        WHEN gamma_email = -1 THEN
            cast(0.0 as float8)
        WHEN gamma_email = 4 THEN
            cast(8.085345696827433 as float8)
        WHEN gamma_email = 3 THEN
            cast(7.952796397293673 as float8)
        WHEN gamma_email = 2 THEN
            cast(7.9253905733462275 as float8)
        WHEN gamma_email = 1 THEN
            cast(4.35549532948094 as float8)
        WHEN gamma_email = 0 THEN
            cast(-6.4162766081139635 as float8)
    END as mw_email,
    CASE
        WHEN gamma_email = -1 THEN cast(0 as float8)
        WHEN gamma_email = 4 THEN
            CASE
                WHEN coalesce("tf_email_l", "tf_email_r") is not null THEN
                    cast(1.0 as float8) * (
                        cast(-8.939181673514824 as float8) - log2(greatest(coalesce("tf_email_l", "tf_email_r"), coalesce("tf_email_r", "tf_email_l"), cast(0.0 as float8)))
                    )
                ELSE cast(0 as float8)
            END
        WHEN gamma_email = 3 THEN cast(0 as float8)
        WHEN gamma_email = 2 THEN cast(0 as float8)
        WHEN gamma_email = 1 THEN cast(0 as float8)
        WHEN gamma_email = 0 THEN cast(0 as float8)
    END as mw_tf_adj_email,
    match_key
    from __splink__df_comparison_vectors
    
