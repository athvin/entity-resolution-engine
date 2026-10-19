INSERT INTO public.schedules (id, org_id, kind, params, cron, source)
VALUES ($schedule_id, $org_id, $kind, $params, $cron, 'api')
