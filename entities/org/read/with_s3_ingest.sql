SELECT
  id,
  name,
  config_path,
  env,
  drop_root,
  s3_ingest_bucket,
  s3_ingest_prefix,
  active_config_version,
  state,
  max_concurrent_jobs,
  created_at,
  updated_at
FROM public.orgs
WHERE s3_ingest_bucket IS NOT NULL
  AND state = 'active'
ORDER BY name
