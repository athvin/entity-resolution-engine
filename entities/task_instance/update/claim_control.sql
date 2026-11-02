UPDATE public.task_instances
SET state = 'running', started_at = now(), heartbeat_at = now(), owner_epoch = $owner_epoch,
    updated_at = now()
WHERE id = (
  SELECT t.id FROM public.task_instances t
  JOIN public.jobs j ON j.id = t.job_id
  WHERE t.live IS TRUE AND t.kind = 'control'
    AND t.state IN ('scheduled', 'up_for_retry')
    AND (t.not_before IS NULL OR t.not_before <= now())
    AND (j.state = 'running' OR (j.state = 'canceling' AND t.teardown))
  ORDER BY j.priority DESC, j.id, t.seq
  FOR UPDATE OF t SKIP LOCKED
  LIMIT 1)
RETURNING
  id,
  org_id,
  dataset_id,
  job_id,
  task_id,
  seq,
  try_number,
  live,
  archived_reason,
  kind,
  state,
  trigger_rule,
  upstream,
  teardown,
  segment,
  resource_class,
  next_resource_class,
  escalated_from,
  max_tries,
  launches,
  max_launches,
  not_before,
  input,
  output,
  outcome,
  exit_code,
  termination_reason,
  error_class,
  error_detail,
  owner_epoch,
  k8s_job_name,
  pod_name,
  pod_uid,
  node_name,
  image_digest,
  snapshot_start,
  snapshot_end,
  counters,
  trace_context,
  token_hash,
  scheduled_at,
  queued_at,
  started_at,
  heartbeat_at,
  finished_at,
  created_at,
  updated_at
