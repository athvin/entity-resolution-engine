-- templated: __dedupe__representatives
-- physical: __dedupe__representatives_f38ff867e
CREATE TABLE __dedupe__representatives_f38ff867e AS
select distinct
    node_id as node_id,
    node_id as representative,
    0 as stable
from __dedupe__nodes_with_composite_ids_aab284a24
