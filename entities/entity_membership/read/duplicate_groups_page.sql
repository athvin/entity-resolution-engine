SELECT entity_id, count(*) AS member_count
FROM lake.main.entity_membership AT (VERSION => $snapshot)
WHERE ($after IS NULL OR entity_id > $after)
  AND ($only_entity IS NULL OR entity_id = $only_entity)
GROUP BY entity_id
HAVING count(*) >= CASE
         WHEN coalesce($min_members, 2) < 2 THEN 2
         ELSE coalesce($min_members, 2)
       END
   AND ($max_members IS NULL OR count(*) <= $max_members)
ORDER BY entity_id
LIMIT $limit
