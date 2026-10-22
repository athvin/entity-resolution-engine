SELECT id, seq, run_id, entity_id, event_type, details, occurred_at
FROM lake.main.entity_events AT (VERSION => $snapshot)
WHERE run_id = $run_id
  AND ($event_type IS NULL OR event_type = $event_type)
  AND ($after IS NULL OR seq > $after)
ORDER BY seq
LIMIT $limit
