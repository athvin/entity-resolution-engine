UPDATE public.schedules SET enabled = $enabled, updated_at = now()
WHERE id = $schedule_id AND dataset_id = $dataset_id AND source = 'api'
