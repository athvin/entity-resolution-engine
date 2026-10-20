-- templated: __dedupe__nodes_with_composite_ids
-- physical: __dedupe__nodes_with_composite_ids_aab284a24
CREATE TABLE __dedupe__nodes_with_composite_ids_aab284a24 AS
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
)
select
    "unique_id" as node_id
    from __dedupe__concat
