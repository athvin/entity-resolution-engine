SELECT
  schedule_id,
  org,
  kind,
  params,
  cron,
  enabled,
  source,
  last_enqueued_at,
  created_at
FROM public.schedules
WHERE org = $org
ORDER BY created_at
