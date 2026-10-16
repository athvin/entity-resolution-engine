SELECT
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
FROM public.jobs
WHERE org = $org AND idempotency_key = $idempotency_key
