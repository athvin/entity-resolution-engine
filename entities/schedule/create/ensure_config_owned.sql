INSERT INTO public.schedules (id, org_id, dataset_id, kind, params, cron, source)
SELECT $schedule_id, $org_id, $dataset_id, $kind, '{}'::jsonb, $cron, $source
WHERE NOT EXISTS (
  SELECT 1 FROM public.schedules
  WHERE dataset_id = $dataset_id AND source = $source
)
