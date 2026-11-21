INSERT INTO lake.main.record_verifications
  (id, record_key, attribute, active, created_by, created_at, source_action_id, retracted_at, updated_at)
VALUES (sha256('record_verifications:' || $record_key || ':' || coalesce($attribute, '')),
        $record_key, $attribute, true, $created_by, $created_at::timestamp,
        $source_action_id, NULL, $created_at::timestamp)
