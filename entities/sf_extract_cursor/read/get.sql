SELECT
  id,
  org_id,
  dataset_id,
  source,
  object,
  watermark,
  delivery_id,
  rows_extracted,
  created_at,
  updated_at
FROM public.sf_extract_cursors
WHERE dataset_id = $dataset_id AND source = $source
