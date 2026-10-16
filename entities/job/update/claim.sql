UPDATE public.jobs
SET state = 'dispatching', started_at = now(), updated_at = now()
WHERE job_id = (
  SELECT j.job_id FROM public.jobs j
  WHERE j.state = 'queued' AND (j.not_before IS NULL OR j.not_before <= now())
    AND NOT EXISTS (
      SELECT 1 FROM public.jobs a
      WHERE a.org = j.org AND a.state IN ('dispatching', 'running', 'canceling'))
  ORDER BY j.priority DESC, j.job_id
  FOR UPDATE SKIP LOCKED
  LIMIT 1)
RETURNING
  job_id,
  org,
  kind,
  params,
  state,
  priority,
  idempotency_key,
  run_id,
  attempt,
  max_attempts,
  not_before,
  created_by,
  schedule_id,
  resource_class,
  trace_context,
  created_at,
  updated_at,
  started_at,
  finished_at
