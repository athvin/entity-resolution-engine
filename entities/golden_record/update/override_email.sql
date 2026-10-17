UPDATE lake.main.golden_records SET email = (
  SELECT o.value FROM lake.main.golden_overrides o
  WHERE o.entity_id = golden_records.entity_id AND o.attribute = 'email')
WHERE golden_records.entity_id IN (
  SELECT entity_id FROM lake.main.golden_overrides WHERE attribute = 'email')
