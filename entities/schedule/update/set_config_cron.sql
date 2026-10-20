UPDATE public.schedules SET cron = $cron, updated_at = now()
WHERE org_id = $org_id AND source = $source AND cron <> $cron
