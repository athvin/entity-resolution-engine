SELECT source_system, source_record_id, record_key, assigned_at, run_id
FROM lake.main.entity_membership AT (VERSION => $snapshot)
WHERE entity_id = $entity_id
ORDER BY record_key
