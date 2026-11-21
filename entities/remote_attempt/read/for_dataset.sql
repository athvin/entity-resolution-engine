SELECT id,
  org_id,
  dataset_id,
  job_id,
  task_instance_id,
  launch,
  binding_generation,
  namespace,
  session_id,
  request_id,
  commit_request_id,
  phase,
  released_at,
  created_at,
  updated_at
FROM public.remote_attempts
WHERE dataset_id = $dataset_id AND released_at IS NULL
