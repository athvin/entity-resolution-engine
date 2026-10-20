INSERT INTO public.schedules (id, org_id, kind, params, cron, source)
SELECT $schedule_id, $org_id, $kind, '{}'::jsonb, $cron, $source
WHERE NOT EXISTS (
  SELECT 1 FROM public.schedules
  WHERE org_id = $org_id AND source = $source
)
