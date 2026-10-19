INSERT INTO lake.main.ingest_batches
  (ingest_batch_id, run_id, source_system, path, new_count, changed_count,
   unchanged_count, tombstone_count, resurrected_count, full_refresh_keys, created_at,
   updated_at)
VALUES ($ingest_batch_id, $run_id, $source_system, $path, $new_count, $changed_count,
  $unchanged_count, $tombstone_count, $resurrected_count, $full_refresh_keys, $created_at::timestamp,
  $created_at::timestamp)
