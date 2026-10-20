-- templated: __dedupe__root_rows
-- physical: __dedupe__root_rows_d0a1aa37a
CREATE TABLE __dedupe__root_rows_d0a1aa37a AS
select count(*) as count_of_edges_needing_processing
from __dedupe__filtered_neighbours_2_c8c92fe41
