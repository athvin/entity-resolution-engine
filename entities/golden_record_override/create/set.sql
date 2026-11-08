INSERT INTO lake.main.golden_record_overrides
  (id, record_key, attribute, value, created_by, created_at, source_action_id, updated_at)
VALUES (sha256('golden_record_overrides:' || $record_key || ':' || $attribute),
        $record_key, $attribute, $value, $created_by, $created_at::timestamp,
        $source_action_id, $created_at::timestamp)
