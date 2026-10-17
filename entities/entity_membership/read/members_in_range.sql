SELECT entity_id, source_system, source_record_id, record_key
FROM lake.main.entity_membership AT (VERSION => $snapshot)
WHERE entity_id >= $first AND entity_id <= $last
ORDER BY entity_id, record_key
