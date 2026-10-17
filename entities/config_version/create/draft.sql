INSERT INTO public.config_versions (org, version, yaml, config_hash, created_by)
SELECT $org, coalesce(max(version), 0) + 1, $yaml, $config_hash, $created_by
FROM public.config_versions WHERE org = $org
RETURNING
  org,
  version,
  yaml,
  config_hash,
  state,
  tier,
  created_by,
  created_at,
  published_at
