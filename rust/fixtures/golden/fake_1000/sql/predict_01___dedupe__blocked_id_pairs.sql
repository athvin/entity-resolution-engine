-- templated: __dedupe__blocked_id_pairs
-- physical: __dedupe__blocked_id_pairs_9732b2e19
CREATE TABLE __dedupe__blocked_id_pairs_9732b2e19 AS
select
'0' as match_key,
l."unique_id" as join_key_l,
r."unique_id" as join_key_r
from __dedupe__concat as l
inner join __dedupe__concat as r
on
((l."first_name" = r."first_name") AND (l."city" = r."city"))
where l."unique_id" < r."unique_id"

UNION ALL

select
'1' as match_key,
l."unique_id" as join_key_l,
r."unique_id" as join_key_r
from __dedupe__concat as l
inner join __dedupe__concat as r
on
(l."surname" = r."surname")
where l."unique_id" < r."unique_id"
AND NOT (coalesce(((l."first_name" = r."first_name") AND (l."city" = r."city")),false))
