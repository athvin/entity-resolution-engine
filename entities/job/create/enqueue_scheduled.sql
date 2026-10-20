INSERT INTO public.jobs
  (id, org_id, kind, priority, idempotency_key, params, schedule_id, created_by)
VALUES
  ($job_id, $org_id, $kind, $priority, $idempotency_key, coalesce($params, '{}'::jsonb),
   $schedule_id, 'scheduler')
ON CONFLICT (org_id, idempotency_key) WHERE idempotency_key IS NOT NULL
  DO UPDATE SET kind = jobs.kind, updated_at = now()
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
  not_before,
  created_by,
  schedule_id,
  resource_class,
  trace_context,
  created_at,
  updated_at,
  started_at,
  finished_at
