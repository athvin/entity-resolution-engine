-- templated: __dedupe__concat_with_tf
-- physical: __dedupe__concat_with_tf_ca8d2bcf3
CREATE TABLE __dedupe__concat_with_tf_ca8d2bcf3 AS

    select
    __dedupe__concat.*,
    __dedupe__tf_email_631ddbc0a."tf_email",
    __dedupe__tf_surname_41b4a4bab."tf_surname",
    __dedupe__tf_first_name_4fee53224."tf_first_name",
    __dedupe__tf_city_b487f6ea2."tf_city"
    from __dedupe__concat
    left join __dedupe__tf_email_631ddbc0a on __dedupe__concat."email" = __dedupe__tf_email_631ddbc0a."email"
left join __dedupe__tf_surname_41b4a4bab on __dedupe__concat."surname" = __dedupe__tf_surname_41b4a4bab."surname"
left join __dedupe__tf_first_name_4fee53224 on __dedupe__concat."first_name" = __dedupe__tf_first_name_4fee53224."first_name"
left join __dedupe__tf_city_b487f6ea2 on __dedupe__concat."city" = __dedupe__tf_city_b487f6ea2."city"
    
