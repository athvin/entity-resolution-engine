-- templated: __splink__root_rows
-- physical: __splink__root_rows_d0a1aa37a
CREATE TABLE __splink__root_rows_d0a1aa37a AS
select count(*) as count_of_edges_needing_processing
from __splink__filtered_neighbours_2_c8c92fe41
