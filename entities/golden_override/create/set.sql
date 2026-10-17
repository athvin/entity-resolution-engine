INSERT INTO lake.main.golden_overrides
  (entity_id, attribute, value, created_by, created_at, source_action_id)
VALUES ($entity_id, $attribute, $value, $created_by, $created_at::timestamp, $source_action_id)
