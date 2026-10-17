SELECT entity_id, given_name, family_name, email
FROM lake.main.golden_records AT (VERSION => $snapshot)
WHERE entity_id >= $first AND entity_id <= $last
