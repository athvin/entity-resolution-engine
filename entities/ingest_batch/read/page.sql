SELECT id, run_id, source_system, path, new_count, changed_count,
       unchanged_count, tombstone_count, resurrected_count, full_refresh_keys,
       created_at
FROM lake.main.ingest_batches AT (VERSION => $snapshot)
WHERE ($source IS NULL OR source_system = $source)
  AND ($after IS NULL OR id > $after)
ORDER BY id
LIMIT $limit
