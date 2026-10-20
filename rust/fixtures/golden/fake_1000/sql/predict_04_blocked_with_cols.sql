-- templated: blocked_with_cols
-- physical: blocked_with_cols_fe8b8962f
CREATE TABLE blocked_with_cols_fe8b8962f AS

    select
    "l"."unique_id" AS "unique_id_l",
    "r"."unique_id" AS "unique_id_r",
    "l"."first_name" AS "first_name_l",
    "r"."first_name" AS "first_name_r",
    "l"."tf_first_name" AS "tf_first_name_l",
    "r"."tf_first_name" AS "tf_first_name_r",
    "l"."surname" AS "surname_l",
    "r"."surname" AS "surname_r",
    "l"."tf_surname" AS "tf_surname_l",
    "r"."tf_surname" AS "tf_surname_r",
    "l"."dob" AS "dob_l",
    "r"."dob" AS "dob_r",
    "l"."city" AS "city_l",
    "r"."city" AS "city_r",
    "l"."tf_city" AS "tf_city_l",
    "r"."tf_city" AS "tf_city_r",
    "l"."email" AS "email_l",
    "r"."email" AS "email_r",
    "l"."tf_email" AS "tf_email_l",
    "r"."tf_email" AS "tf_email_r",
    b.match_key
    from __dedupe__blocked_id_pairs_9732b2e19 as b
    inner join __dedupe__concat_with_tf as l
    on l."unique_id" = b.join_key_l
    inner join __dedupe__concat_with_tf as r
    on r."unique_id" = b.join_key_r
    
