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
  created_at,
  updated_at
FROM public.orgs
WHERE id = $id
