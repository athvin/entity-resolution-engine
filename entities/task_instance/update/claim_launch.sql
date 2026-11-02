UPDATE public.task_instances
SET state = 'queued', queued_at = now(), owner_epoch = $owner_epoch,
    launches = task_instances.launches + 1,
    -- The Job name is a pure function of the attempt key and the launch count
    -- (§4.2) — derived HERE because only the statement knows the incremented
    -- count. The caller override exists for the in-process reporter's
    -- synthetic 'inproc' identity.
    k8s_job_name = COALESCE($k8s_job_name,
        'er-t-' || substr(task_instances.id, 1, 20) || '-l' || (task_instances.launches + 1)::text),
    token_hash = $token_hash, updated_at = now()
WHERE id = (
  SELECT t.id FROM public.task_instances t
  JOIN public.jobs j ON j.id = t.job_id
  WHERE t.live IS TRUE AND t.kind = 'pod'
    AND t.state IN ('scheduled', 'up_for_retry')
    AND (t.not_before IS NULL OR t.not_before <= now())
    AND (j.state = 'running' OR (j.state = 'canceling' AND t.teardown))
    AND NOT EXISTS (SELECT 1 FROM public.task_instances a
                    WHERE a.org_id = t.org_id AND a.live IS TRUE AND a.kind = 'pod'
                      AND a.state IN ('queued', 'running', 'restarting', 'canceling'))
    AND $in_flight_cap > (SELECT count(*) FROM public.task_instances c
                          WHERE c.live IS TRUE AND c.kind = 'pod' AND c.state IN ('queued', 'running')
                            AND c.k8s_job_name IS DISTINCT FROM 'inproc')
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
