UPDATE public.schedules SET enabled = $enabled
WHERE schedule_id = $schedule_id AND org = $org AND source = 'api'
