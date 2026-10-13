-- templated: __splink__filtered_neighbours_1
-- physical: __splink__filtered_neighbours_1_12318bcb6
CREATE TABLE __splink__filtered_neighbours_1_12318bcb6 AS
select
    l.representative as node_rep,
    n.node_id,
    n.neighbour,
    r.representative as neighbour_rep
from __splink__df_neighbours_3192c2212 as n
left join __splink__representatives_1_225b6c241 as l
on l.node_id = n.node_id
left join __splink__representatives_1_225b6c241 as r
on n.neighbour = r.node_id
where l.representative <> r.representative
