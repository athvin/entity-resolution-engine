INSERT INTO lake.main.record_overlays
  (id, record_key, kind, active, created_by, created_at, source_action_id, retracted_at, updated_at)
VALUES (sha256('record_overlays:' || $record_key || ':' || $kind),
        $record_key, $kind, true, $created_by, $created_at::timestamp,
        $source_action_id, NULL, $created_at::timestamp)
