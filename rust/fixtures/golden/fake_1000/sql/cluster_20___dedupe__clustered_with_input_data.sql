-- templated: __dedupe__clustered_with_input_data
-- physical: __dedupe__clustered_with_input_data_19498f8f2
CREATE TABLE __dedupe__clustered_with_input_data_19498f8f2 AS
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
    cc.cluster_id,
    "unique_id", "first_name", "surname", "dob", "city", "email", "cluster"
from __dedupe__clustering_output_final_a0fa832b4 as cc
left join __dedupe__concat
on cc.node_id = "unique_id"
