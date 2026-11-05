SELECT
  id,
  org_id, dataset_id,
  source,
  object_key,
  etag,
  size_bytes,
  delivery_id,
  created_at,
  updated_at
FROM public.s3_ingest_cursors
WHERE dataset_id = $dataset_id
ORDER BY source
