INSERT INTO public.s3_ingest_cursors
  (id, org_id, source, object_key, etag, size_bytes, delivery_id)
VALUES
  ($cursor_id, $org_id, $source, $object_key, $etag, $size_bytes, $delivery_id)
ON CONFLICT (org_id, source) DO UPDATE SET
  object_key = EXCLUDED.object_key,
  etag = EXCLUDED.etag,
  size_bytes = EXCLUDED.size_bytes,
  delivery_id = EXCLUDED.delivery_id,
  updated_at = now()
RETURNING id, org_id, source, object_key, etag, size_bytes, delivery_id,
          created_at, updated_at
