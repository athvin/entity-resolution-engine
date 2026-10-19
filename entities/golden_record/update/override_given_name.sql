UPDATE lake.main.golden_records SET given_name = (
  SELECT o.value FROM lake.main.golden_overrides o
  WHERE o.entity_id = golden_records.id AND o.attribute = 'given_name'),
    updated_at = $updated_at::timestamp
WHERE golden_records.id IN (
  SELECT entity_id FROM lake.main.golden_overrides WHERE attribute = 'given_name')
