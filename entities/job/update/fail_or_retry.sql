UPDATE public.jobs
SET state = CASE WHEN $retryable AND attempt < max_attempts THEN 'queued' ELSE 'failed' END,
    -- Exponential backoff from the attempt already made, capped so a job cannot
    -- drift hours into the future: 5s, 10s, 20s, … 300s.
    not_before = CASE WHEN $retryable AND attempt < max_attempts
                      THEN now() + make_interval(
                             secs => least(300, 5 * power(2, greatest(attempt - 1, 0)))::int)
                      ELSE not_before END,
    -- A retry is a fresh attempt, so the previous attempt's stamps are cleared;
    -- a terminal failure keeps them for the audit trail.
    started_at = CASE WHEN $retryable AND attempt < max_attempts THEN NULL ELSE started_at END,
    finished_at = CASE WHEN $retryable AND attempt < max_attempts THEN NULL ELSE now() END,
    error_class = $error_class,
    error_detail = $error_detail,
    updated_at = now()
WHERE id = $job_id
RETURNING
  id,
  org_id,
  kind,
  params,
  state,
  priority,
  idempotency_key,
  run_id,
  attempt,
  max_attempts,
  error_class,
  error_detail,
  not_before,
  created_by,
  schedule_id,
  resource_class,
  trace_context,
  created_at,
  updated_at,
  started_at,
  finished_at
