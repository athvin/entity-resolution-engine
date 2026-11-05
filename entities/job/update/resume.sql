UPDATE public.jobs SET state = 'queued', not_before = now(), updated_at = now(),
    finished_at = NULL
WHERE org_id = $org_id AND id = $job_id AND state IN ('failed', 'canceled')
