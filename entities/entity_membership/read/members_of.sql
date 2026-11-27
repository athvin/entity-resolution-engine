SELECT entity_id, record_key, source_system, source_record_id
FROM lake.main.entity_membership AT (VERSION => $snapshot)
WHERE entity_id IN (SELECT unnest(string_split($entity_ids, chr(31))))
ORDER BY entity_id, record_key
