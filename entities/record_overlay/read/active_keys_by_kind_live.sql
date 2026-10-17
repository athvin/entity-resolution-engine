SELECT record_key
FROM lake.main.record_overlays
WHERE kind = $kind AND active
