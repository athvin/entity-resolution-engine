UPDATE public.remote_attempts
SET session_id = $session_id, phase = 'active', updated_at = now()
WHERE id = $id AND task_instance_id = $task_instance_id
  AND released_at IS NULL AND phase = 'opening' AND session_id IS NULL
  AND EXISTS (SELECT 1 FROM public.task_instances t
    WHERE t.id = remote_attempts.task_instance_id AND t.launches = remote_attempts.launch
      AND t.pod_uid = $pod_uid AND t.live IS TRUE AND t.state = 'running')
