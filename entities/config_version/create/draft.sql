INSERT INTO public.config_versions (id, org_id, version, yaml, config_hash, created_by)
SELECT $id, $org_id, coalesce(max(version), 0) + 1, $yaml, $config_hash, $created_by
FROM public.config_versions WHERE org_id = $org_id
RETURNING
  id,
  org_id,
  version,
  yaml,
  config_hash,
  state,
  tier,
  created_by,
  created_at,
  published_at,
  updated_at
