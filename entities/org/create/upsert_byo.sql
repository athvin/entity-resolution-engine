INSERT INTO public.orgs
  (id, name, config_path, env, drop_root, s3_ingest_bucket, s3_ingest_prefix, state)
VALUES
  ($id, $name, $config_path, $env, $drop_root, $s3_ingest_bucket,
   $s3_ingest_prefix, 'active')
ON CONFLICT (name) DO UPDATE SET config_path = EXCLUDED.config_path,
  env = EXCLUDED.env, drop_root = EXCLUDED.drop_root,
  s3_ingest_bucket = EXCLUDED.s3_ingest_bucket,
  s3_ingest_prefix = EXCLUDED.s3_ingest_prefix, updated_at = now()
