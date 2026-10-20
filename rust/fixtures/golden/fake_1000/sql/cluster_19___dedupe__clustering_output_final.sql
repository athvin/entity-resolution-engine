-- templated: __dedupe__clustering_output_final
-- physical: __dedupe__clustering_output_final_a0fa832b4
CREATE TABLE __dedupe__clustering_output_final_a0fa832b4 AS
select node_id as node_id, representative as cluster_id
            from __dedupe__representatives_stable_1_79ccd7308

UNION ALL

select node_id as node_id, representative as cluster_id
            from __dedupe__representatives_stable_2_9d07fd4b4

UNION ALL

select node_id as node_id, representative as cluster_id
            from __dedupe__representatives_stable_3_9049de322

UNION ALL

select node_id as node_id, representative as cluster_id
            from __dedupe__representatives_unstable_3_bf56c8897
