SELECT
  org,
  version,
  yaml,
  config_hash,
  state,
  tier,
  created_by,
  created_at,
  published_at
FROM public.config_versions
WHERE org = $org AND version = $version
