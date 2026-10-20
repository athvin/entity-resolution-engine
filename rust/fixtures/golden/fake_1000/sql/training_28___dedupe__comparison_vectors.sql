-- templated: __dedupe__comparison_vectors
-- physical: __dedupe__comparison_vectors_8ce534105
CREATE TABLE __dedupe__comparison_vectors_8ce534105 AS
WITH

__dedupe__concat as (
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

__dedupe__concat_with_tf as (
    select
        __dedupe__concat.*,
        __dedupe__tf_email_631ddbc0a."tf_email",
        __dedupe__tf_city_b487f6ea2."tf_city"
        from __dedupe__concat
        left join __dedupe__tf_email_631ddbc0a on __dedupe__concat."email" = __dedupe__tf_email_631ddbc0a."email"
    left join __dedupe__tf_city_b487f6ea2 on __dedupe__concat."city" = __dedupe__tf_city_b487f6ea2."city"
), 

blocked_with_cols as (
    select
    "l"."unique_id" AS "unique_id_l",
    "r"."unique_id" AS "unique_id_r",
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
    "l"."surname" AS "surname_l",
    "r"."surname" AS "surname_r",
    "l"."first_name" AS "first_name_l",
    "r"."first_name" AS "first_name_r",
    b.match_key
    from __dedupe__blocked_id_pairs_71d3c1477 as b
    inner join __dedupe__concat_with_tf as l
    on l."unique_id" = b.join_key_l
    inner join __dedupe__concat_with_tf as r
    on r."unique_id" = b.join_key_r
)
select
"unique_id_l",
"unique_id_r",
CASE
    WHEN "dob_l" IS NULL OR "dob_r" IS NULL THEN -1
    WHEN "dob_l" = "dob_r" THEN 2
    WHEN levenshtein("dob_l", "dob_r") <= 1 THEN 1
    ELSE 0
END as gamma_dob,
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
