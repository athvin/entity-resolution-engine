SELECT DISTINCT source_record_id
FROM lake.main.raw_records
WHERE source_system = $source AND ingest_batch_id <> $batch
  AND source_record_id NOT IN (
    SELECT source_record_id
    FROM lake.main.raw_records
    WHERE source_system = $source AND ingest_batch_id = $batch
  )
ORDER BY source_record_id
