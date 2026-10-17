SELECT attribute, record_key, source_system, source_record_id, rule
FROM lake.main.golden_lineage AT (VERSION => $snapshot)
WHERE entity_id = $entity_id
ORDER BY attribute, record_key
