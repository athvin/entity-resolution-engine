WITH prior AS (
  SELECT source_record_id, content_hash,
         row_number() OVER (PARTITION BY source_record_id
           ORDER BY ingested_at DESC, ingest_batch_id DESC) AS rn
  FROM lake.main.raw_records
  WHERE source_system = $source AND ingest_batch_id <> $batch
),
prior_latest AS (SELECT source_record_id, content_hash FROM prior WHERE rn = 1),
this_batch AS (
  SELECT source_record_id, content_hash
  FROM lake.main.raw_records
  WHERE source_system = $source AND ingest_batch_id = $batch
)
SELECT
  count(*) FILTER (WHERE tb.content_hash <> $tombstone_hash
                     AND p.source_record_id IS NULL) AS new_count,
  count(*) FILTER (WHERE tb.content_hash <> $tombstone_hash
                     AND p.content_hash = $tombstone_hash) AS resurrected_count,
  count(*) FILTER (WHERE tb.content_hash <> $tombstone_hash
                     AND p.content_hash = tb.content_hash) AS unchanged_count,
  count(*) FILTER (WHERE tb.content_hash <> $tombstone_hash
                     AND p.source_record_id IS NOT NULL
                     AND p.content_hash <> $tombstone_hash
                     AND p.content_hash <> tb.content_hash) AS changed_count,
  count(*) FILTER (WHERE tb.content_hash = $tombstone_hash
                     AND p.source_record_id IS NOT NULL
                     AND p.content_hash <> $tombstone_hash) AS tombstone_count
FROM this_batch tb
LEFT JOIN prior_latest p ON tb.source_record_id = p.source_record_id
