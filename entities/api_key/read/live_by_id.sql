SELECT
  id,
  org_id,
  role,
  key_hash,
  created_at,
  revoked_at,
  updated_at
FROM public.api_keys
WHERE id = $key_id AND revoked_at IS NULL
