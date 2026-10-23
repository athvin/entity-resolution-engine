SELECT entity_id
FROM lake.main.entity_membership AT (VERSION => $snapshot)
WHERE record_key = $record_key
