SELECT
  id,
  org_id,
  name,
  display_name,
  entity_type,
  config_path,
  env,
  drop_root,
  s3_ingest_bucket,
  s3_ingest_prefix,
  active_config_version,
  state,
  created_at,
  updated_at
FROM public.datasets
WHERE org_id = $org_id AND name = $name
