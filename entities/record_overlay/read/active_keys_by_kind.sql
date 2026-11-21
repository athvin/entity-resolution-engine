SELECT record_key
FROM lake.main.record_overlays AT (VERSION => $snapshot)
WHERE kind = $kind AND active
