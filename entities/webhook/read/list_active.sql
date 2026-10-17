SELECT
  webhook_id,
  org,
  url,
  events,
  secret,
  active,
  created_at
FROM public.webhooks
WHERE org = $org AND active
ORDER BY created_at
