INSERT INTO lake.main.record_overlays
  (record_key, kind, active, created_by, created_at, source_action_id, retracted_at)
VALUES ($record_key, $kind, true, $created_by, $created_at::timestamp, $source_action_id, NULL)
