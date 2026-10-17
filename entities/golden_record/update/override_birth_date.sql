UPDATE lake.main.golden_records SET birth_date = (
  SELECT CAST(o.value AS DATE) FROM lake.main.golden_overrides o
  WHERE o.entity_id = golden_records.entity_id AND o.attribute = 'birth_date')
WHERE golden_records.entity_id IN (
  SELECT entity_id FROM lake.main.golden_overrides WHERE attribute = 'birth_date')
