INSERT INTO lake.main.golden_overrides
  (id, entity_id, attribute, value, created_by, created_at, source_action_id, updated_at)
VALUES (sha256('golden_overrides:' || $entity_id || ':' || $attribute),
        $entity_id, $attribute, $value, $created_by, $created_at::timestamp,
        $source_action_id, $created_at::timestamp)
