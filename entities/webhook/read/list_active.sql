SELECT
  id,
  org_id,
  url,
  events,
  secret,
  active,
  created_at,
  updated_at
FROM public.webhooks
WHERE org_id = $org_id AND active
ORDER BY created_at
