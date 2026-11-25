SELECT record_key, source_system, source_record_id, entity_id
FROM lake.main.entity_membership AT (VERSION => $snapshot)
WHERE ($after IS NULL OR record_key > $after)
ORDER BY record_key
LIMIT $limit
