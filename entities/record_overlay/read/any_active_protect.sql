SELECT CAST(1 AS BIGINT) AS present
FROM lake.main.record_overlays
WHERE kind = 'protect' AND active
LIMIT 1
