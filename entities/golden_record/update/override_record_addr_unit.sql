UPDATE lake.main.golden_records SET addr_unit = (
  SELECT o.value FROM lake.main.golden_record_overrides o
  JOIN lake.main.entity_membership m
    ON m.record_key = o.record_key AND m.entity_id = golden_records.id
  WHERE o.attribute = 'addr_unit'
  ORDER BY o.created_at DESC, o.record_key LIMIT 1),
    updated_at = $updated_at::timestamp
WHERE golden_records.id IN (
  SELECT m.entity_id FROM lake.main.golden_record_overrides o
  JOIN lake.main.entity_membership m ON m.record_key = o.record_key
  WHERE o.attribute = 'addr_unit')
