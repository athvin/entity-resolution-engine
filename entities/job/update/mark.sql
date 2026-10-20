UPDATE public.jobs
SET state = $state,
    updated_at = now(),
    -- A job that finally succeeds must not still show the failure that made it
    -- retry; the UI renders error_class whenever it is present.
    error_class = CASE WHEN $state = 'succeeded' THEN NULL ELSE error_class END,
    error_detail = CASE WHEN $state = 'succeeded' THEN NULL ELSE error_detail END,
    finished_at = CASE WHEN $state IN ('succeeded', 'failed', 'canceled') THEN now() ELSE finished_at END
WHERE id = $job_id
