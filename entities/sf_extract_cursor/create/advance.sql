INSERT INTO public.sf_extract_cursors
  (id, org_id, dataset_id, source, object, watermark, delivery_id, rows_extracted)
VALUES
  ($cursor_id, $org_id, $dataset_id, $source, $object, $watermark, $delivery_id, $rows_extracted)
ON CONFLICT (dataset_id, source) DO UPDATE SET
  object = EXCLUDED.object,
  watermark = COALESCE(EXCLUDED.watermark, public.sf_extract_cursors.watermark),
  delivery_id = EXCLUDED.delivery_id,
  rows_extracted = EXCLUDED.rows_extracted,
  updated_at = now()
RETURNING id, org_id, dataset_id, source, object, watermark, delivery_id, rows_extracted,
          created_at, updated_at
