-- templated: __splink__df_comparison_vectors
-- physical: __splink__df_comparison_vectors_c582851d0
CREATE TABLE __splink__df_comparison_vectors_c582851d0 AS

    select
    "unique_id_l",
    "unique_id_r",
    "first_name_l",
    "first_name_r",
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
    "surname_l",
    "surname_r",
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
    "dob_l",
    "dob_r",
    CASE
        WHEN "dob_l" IS NULL OR "dob_r" IS NULL THEN -1
        WHEN "dob_l" = "dob_r" THEN 2
        WHEN levenshtein("dob_l", "dob_r") <= 1 THEN 1
        ELSE 0
    END as gamma_dob,
    "city_l",
    "city_r",
    CASE
        WHEN "city_l" IS NULL OR "city_r" IS NULL THEN -1
        WHEN "city_l" = "city_r" THEN 1
        ELSE 0
    END as gamma_city,
    "tf_city_l",
    "tf_city_r",
    "email_l",
    "email_r",
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
    
