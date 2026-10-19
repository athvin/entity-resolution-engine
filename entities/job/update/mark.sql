UPDATE public.jobs
SET state = $state,
    updated_at = now(),
    finished_at = CASE WHEN $state IN ('succeeded', 'failed', 'canceled') THEN now() ELSE finished_at END
WHERE id = $job_id
