SELECT
  id,
  org_id,
  name,
  instance_url,
  auth_kind,
  client_id,
  client_secret,
  api_version,
  active,
  created_at,
  updated_at
FROM public.salesforce_connections
WHERE org_id = $org_id AND active
ORDER BY created_at
