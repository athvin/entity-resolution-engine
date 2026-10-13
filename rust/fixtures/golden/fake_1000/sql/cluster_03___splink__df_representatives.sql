-- templated: __splink__df_representatives
-- physical: __splink__df_representatives_f38ff867e
CREATE TABLE __splink__df_representatives_f38ff867e AS
select distinct
    node_id as node_id,
    node_id as representative,
    0 as stable
from __splink__df_nodes_with_composite_ids_aab284a24
