UPDATE lake.main.golden_records SET phone_e164 = (
  SELECT o.value FROM lake.main.golden_overrides o
  WHERE o.entity_id = golden_records.entity_id AND o.attribute = 'phone_e164')
WHERE golden_records.entity_id IN (
  SELECT entity_id FROM lake.main.golden_overrides WHERE attribute = 'phone_e164')
