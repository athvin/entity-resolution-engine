-- templated: __dedupe__root_rows
-- physical: __dedupe__root_rows_c654d9587
CREATE TABLE __dedupe__root_rows_c654d9587 AS
select count(*) as count_of_edges_needing_processing
from __dedupe__filtered_neighbours_1_12318bcb6
