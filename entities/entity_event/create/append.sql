INSERT INTO lake.main.entity_events
  (id, seq, run_id, entity_id, event_type, details, details_hash, occurred_at,
   created_at, updated_at)
VALUES ($event_id, $seq, $run_id, $entity_id, $event_type, $details::json, $details_hash, $occurred_at::timestamp,
  $occurred_at::timestamp, $occurred_at::timestamp)
