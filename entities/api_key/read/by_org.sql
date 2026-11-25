SELECT
  id,
  org_id,
  role,
  key_hash,
  created_at,
  revoked_at,
  updated_at
FROM public.api_keys
WHERE org_id = $org_id
ORDER BY created_at DESC, id
