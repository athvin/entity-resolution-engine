SELECT event_id, seq, run_id, event_type, details, occurred_at
FROM lake.main.entity_events AT (VERSION => $snapshot)
WHERE entity_id = $entity_id
ORDER BY occurred_at, seq
