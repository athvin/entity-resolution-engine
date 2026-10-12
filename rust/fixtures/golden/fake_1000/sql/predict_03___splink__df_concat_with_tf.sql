-- templated: __splink__df_concat_with_tf
-- physical: __splink__df_concat_with_tf_ca8d2bcf3
CREATE TABLE __splink__df_concat_with_tf_ca8d2bcf3 AS

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
    
