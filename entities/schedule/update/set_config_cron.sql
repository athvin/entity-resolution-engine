UPDATE public.schedules SET cron = $cron, updated_at = now()
WHERE dataset_id = $dataset_id AND source = $source AND cron <> $cron
