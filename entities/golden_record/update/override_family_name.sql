UPDATE lake.main.golden_records SET family_name = (
  SELECT o.value FROM lake.main.golden_overrides o
  WHERE o.entity_id = golden_records.id AND o.attribute = 'family_name'),
    updated_at = $updated_at::timestamp
WHERE golden_records.id IN (
  SELECT entity_id FROM lake.main.golden_overrides WHERE attribute = 'family_name')
