SELECT a.id,
  a.org_id,
  a.dataset_id,
  a.job_id,
  a.task_instance_id,
  a.launch,
  a.binding_generation,
  a.namespace,
  a.session_id,
  a.request_id,
  a.commit_request_id,
  a.phase,
  a.released_at,
  a.created_at,
  a.updated_at
FROM public.remote_attempts a
WHERE a.released_at IS NULL
  AND NOT EXISTS (SELECT 1 FROM public.task_instances t
    WHERE t.id = a.task_instance_id AND t.launches = a.launch
      AND t.live IS TRUE AND t.state = 'running')
ORDER BY a.updated_at, a.id
LIMIT 100
