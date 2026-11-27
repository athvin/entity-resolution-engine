SELECT DISTINCT run_id, entity_id
FROM lake.main.entity_events AT (VERSION => $snapshot)
WHERE run_id IN (SELECT unnest(string_split($run_ids, chr(31))))
  AND event_type IN ('created', 'merged', 'member_added', 'split', 'member_removed', 'retired')
ORDER BY run_id, entity_id
