SELECT record_key
FROM lake.main.entity_membership AT (VERSION => $snapshot)
WHERE entity_id = $entity_id
ORDER BY record_key
