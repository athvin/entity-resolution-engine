SELECT
  id,
  org_id,
  kind,
  params,
  cron,
  enabled,
  source,
  last_enqueued_at,
  created_at,
  updated_at
FROM public.schedules
WHERE org_id = $org_id
ORDER BY created_at
