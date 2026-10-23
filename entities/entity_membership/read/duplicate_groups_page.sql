SELECT entity_id, count(*) AS member_count
FROM lake.main.entity_membership AT (VERSION => $snapshot)
WHERE ($after IS NULL OR entity_id > $after)
GROUP BY entity_id
HAVING count(*) >= CASE
         WHEN coalesce($min_members, 2) < 2 THEN 2
         ELSE coalesce($min_members, 2)
       END
ORDER BY entity_id
LIMIT $limit
