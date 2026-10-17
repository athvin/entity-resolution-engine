SELECT entity_id, count(*) AS member_count
FROM lake.main.entity_membership AT (VERSION => $snapshot)
WHERE ($after IS NULL OR entity_id > $after)
GROUP BY entity_id
HAVING count(*) >= 2
ORDER BY entity_id
LIMIT $limit
