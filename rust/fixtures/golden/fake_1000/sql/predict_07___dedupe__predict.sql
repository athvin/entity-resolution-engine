-- templated: __dedupe__predict
-- physical: __dedupe__predict_70bfbe071
CREATE TABLE __dedupe__predict_70bfbe071 AS

    select
    cast(-8.386106589561935 as float8) + mw_first_name + mw_tf_adj_first_name + mw_surname + mw_tf_adj_surname + mw_dob + mw_city + mw_tf_adj_city + mw_email + mw_tf_adj_email as match_weight,
    CASE
            WHEN (cast(-8.386106589561935 as float8) + mw_first_name + mw_tf_adj_first_name + mw_surname + mw_tf_adj_surname + mw_dob + mw_city + mw_tf_adj_city + mw_email + mw_tf_adj_email) >= 0 THEN
                1.0 / (1.0 + POWER(cast(2 as float8), -(cast(-8.386106589561935 as float8) + mw_first_name + mw_tf_adj_first_name + mw_surname + mw_tf_adj_surname + mw_dob + mw_city + mw_tf_adj_city + mw_email + mw_tf_adj_email)))
            ELSE
                POWER(cast(2 as float8), (cast(-8.386106589561935 as float8) + mw_first_name + mw_tf_adj_first_name + mw_surname + mw_tf_adj_surname + mw_dob + mw_city + mw_tf_adj_city + mw_email + mw_tf_adj_email)) /
                (1.0 + POWER(cast(2 as float8), (cast(-8.386106589561935 as float8) + mw_first_name + mw_tf_adj_first_name + mw_surname + mw_tf_adj_surname + mw_dob + mw_city + mw_tf_adj_city + mw_email + mw_tf_adj_email)))
        END as match_probability,
    "unique_id_l",
    "unique_id_r",
    "first_name_l",
    "first_name_r",
    gamma_first_name,
    "tf_first_name_l",
    "tf_first_name_r",
    mw_first_name,
    mw_tf_adj_first_name,
    "surname_l",
    "surname_r",
    gamma_surname,
    "tf_surname_l",
    "tf_surname_r",
    mw_surname,
    mw_tf_adj_surname,
    "dob_l",
    "dob_r",
    gamma_dob,
    mw_dob,
    "city_l",
    "city_r",
    gamma_city,
    "tf_city_l",
    "tf_city_r",
    mw_city,
    mw_tf_adj_city,
    "email_l",
    "email_r",
    gamma_email,
    "tf_email_l",
    "tf_email_r",
    mw_email,
    mw_tf_adj_email,
    match_key
    from __dedupe__match_weight_parts
    
    
