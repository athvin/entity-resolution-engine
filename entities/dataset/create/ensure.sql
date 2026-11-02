INSERT INTO public.datasets
  (id, org_id, name, display_name, entity_type, config_path, env, drop_root,
   s3_ingest_bucket, s3_ingest_prefix, active_config_version, state)
VALUES
  ($id, $org_id, $name, $display_name, $entity_type, $config_path, $env,
   $drop_root, $s3_ingest_bucket, $s3_ingest_prefix, $active_config_version,
   $state)
ON CONFLICT (org_id, name) DO NOTHING
