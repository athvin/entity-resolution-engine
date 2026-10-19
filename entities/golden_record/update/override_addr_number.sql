UPDATE lake.main.golden_records SET addr_number = (
  SELECT o.value FROM lake.main.golden_overrides o
  WHERE o.entity_id = golden_records.id AND o.attribute = 'addr_number'),
    updated_at = $updated_at::timestamp
WHERE golden_records.id IN (
  SELECT entity_id FROM lake.main.golden_overrides WHERE attribute = 'addr_number')
