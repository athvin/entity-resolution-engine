SELECT source_system, source_record_id, payload, is_deleted, ingest_batch_id
FROM (
  SELECT source_system, source_record_id, payload, is_deleted, ingest_batch_id,
         row_number() OVER (
           PARTITION BY source_system, source_record_id
           ORDER BY ingested_at DESC, ingest_batch_id DESC
         ) AS rn
  FROM lake.main.raw_records AT (VERSION => $snapshot)
  WHERE (source_system || chr(30) || source_record_id)
        IN (SELECT unnest(string_split($keys, chr(31))))
)
WHERE rn = 1
ORDER BY source_system, source_record_id
