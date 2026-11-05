SELECT
  id,
  org_id,
  dataset_id,
  version,
  yaml,
  config_hash,
  state,
  tier,
  created_by,
  created_at,
  published_at,
  updated_at
FROM public.config_versions
WHERE dataset_id = $dataset_id AND version = $version
