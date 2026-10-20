SELECT
  s.id,
  s.org_id,
  s.kind,
  s.params,
  s.cron,
  s.enabled,
  s.source,
  s.last_enqueued_at,
  s.created_at,
  s.updated_at
FROM public.schedules s
JOIN public.orgs o ON o.id = s.org_id
WHERE s.enabled AND o.state = 'active'
ORDER BY s.id
