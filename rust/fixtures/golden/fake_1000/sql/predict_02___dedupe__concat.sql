-- templated: __dedupe__concat
-- physical: __dedupe__concat_1dbf7939a
CREATE TABLE __dedupe__concat_1dbf7939a AS

            select
    "unique_id",
    "first_name",
    "surname",
    "dob",
    "city",
    "email",
    "cluster"
            from fake_1000
            
