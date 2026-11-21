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
JOIN public.task_instances t ON t.dataset_id = a.dataset_id
WHERE t.id = $task_instance_id AND a.released_at IS NULL
