-- templated: __splink__df_comparison_vectors
-- physical: __splink__df_comparison_vectors_31fe7218a
CREATE TABLE __splink__df_comparison_vectors_31fe7218a AS
WITH

__splink__df_concat as (
    select
    "unique_id",
    "first_name",
    "surname",
    "dob",
    "city",
    "email",
    "cluster"
            from fake_1000
), 

__splink__df_concat_with_tf as (
    select
        __splink__df_concat.*,
        __splink__df_tf_email_631ddbc0a."tf_email",
        __splink__df_tf_surname_41b4a4bab."tf_surname",
        __splink__df_tf_first_name_4fee53224."tf_first_name",
        __splink__df_tf_city_b487f6ea2."tf_city"
        from __splink__df_concat
        left join __splink__df_tf_email_631ddbc0a on __splink__df_concat."email" = __splink__df_tf_email_631ddbc0a."email"
    left join __splink__df_tf_surname_41b4a4bab on __splink__df_concat."surname" = __splink__df_tf_surname_41b4a4bab."surname"
    left join __splink__df_tf_first_name_4fee53224 on __splink__df_concat."first_name" = __splink__df_tf_first_name_4fee53224."first_name"
    left join __splink__df_tf_city_b487f6ea2 on __splink__df_concat."city" = __splink__df_tf_city_b487f6ea2."city"
), 

blocked_with_cols as (
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
    "l"."city" AS "city_l",
    "r"."city" AS "city_r",
    "l"."tf_city" AS "tf_city_l",
    "r"."tf_city" AS "tf_city_r",
    "l"."email" AS "email_l",
    "r"."email" AS "email_r",
    "l"."tf_email" AS "tf_email_l",
    "r"."tf_email" AS "tf_email_r",
    b.match_key
    from __splink__blocked_id_pairs_e105ce473 as b
    inner join __splink__df_concat_with_tf as l
    on l."unique_id" = b.join_key_l
    inner join __splink__df_concat_with_tf as r
    on r."unique_id" = b.join_key_r
)
select
"unique_id_l",
"unique_id_r",
CASE
    WHEN "first_name_l" IS NULL OR "first_name_r" IS NULL THEN -1
    WHEN "first_name_l" = "first_name_r" THEN 4
    WHEN jaro_winkler_similarity("first_name_l", "first_name_r") >= 0.92 THEN 3
    WHEN jaro_winkler_similarity("first_name_l", "first_name_r") >= 0.88 THEN 2
    WHEN jaro_winkler_similarity("first_name_l", "first_name_r") >= 0.7 THEN 1
    ELSE 0
END as gamma_first_name,
"tf_first_name_l",
"tf_first_name_r",
CASE
    WHEN "surname_l" IS NULL OR "surname_r" IS NULL THEN -1
    WHEN "surname_l" = "surname_r" THEN 4
    WHEN jaro_winkler_similarity("surname_l", "surname_r") >= 0.92 THEN 3
    WHEN jaro_winkler_similarity("surname_l", "surname_r") >= 0.88 THEN 2
    WHEN jaro_winkler_similarity("surname_l", "surname_r") >= 0.7 THEN 1
    ELSE 0
END as gamma_surname,
"tf_surname_l",
"tf_surname_r",
CASE
    WHEN "city_l" IS NULL OR "city_r" IS NULL THEN -1
    WHEN "city_l" = "city_r" THEN 1
    ELSE 0
END as gamma_city,
"tf_city_l",
"tf_city_r",
CASE
    WHEN "email_l" IS NULL OR "email_r" IS NULL THEN -1
    WHEN "email_l" = "email_r" THEN 4
    WHEN NULLIF(regexp_extract("email_l", '^[^@]+', 0), '') = NULLIF(regexp_extract("email_r", '^[^@]+', 0), '') THEN 3
    WHEN jaro_winkler_similarity("email_l", "email_r") >= 0.88 THEN 2
    WHEN jaro_winkler_similarity(NULLIF(regexp_extract("email_l", '^[^@]+', 0), ''), NULLIF(regexp_extract("email_r", '^[^@]+', 0), '')) >= 0.88 THEN 1
    ELSE 0
END as gamma_email,
"tf_email_l",
"tf_email_r",
match_key
from blocked_with_cols
