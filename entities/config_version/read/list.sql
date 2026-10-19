SELECT
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
FROM public.config_versions
WHERE org_id = $org_id
ORDER BY version DESC
