UPDATE public.jobs SET state = 'queued', not_before = now(), updated_at = now(),
    finished_at = NULL,
    params = CASE WHEN run_id IS NULL THEN params
                  ELSE params || jsonb_build_object('resume_run_id', run_id) END
WHERE org = $org AND job_id = $job_id AND state IN ('failed', 'canceled')
