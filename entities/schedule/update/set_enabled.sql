UPDATE public.schedules SET enabled = $enabled, updated_at = now()
WHERE id = $schedule_id AND org_id = $org_id AND source = 'api'
