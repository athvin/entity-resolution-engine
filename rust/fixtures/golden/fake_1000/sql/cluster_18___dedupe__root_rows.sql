-- templated: __dedupe__root_rows
-- physical: __dedupe__root_rows_2731db815
CREATE TABLE __dedupe__root_rows_2731db815 AS
select count(*) as count_of_edges_needing_processing
from __dedupe__filtered_neighbours_3_feb87a854
