UPDATE lake.main.golden_records SET family_name = (
  SELECT o.value FROM lake.main.golden_overrides o
  WHERE o.entity_id = golden_records.entity_id AND o.attribute = 'family_name')
WHERE golden_records.entity_id IN (
  SELECT entity_id FROM lake.main.golden_overrides WHERE attribute = 'family_name')
