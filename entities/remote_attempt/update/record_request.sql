UPDATE public.remote_attempts
SET request_id = $request_id,
    commit_request_id = CASE WHEN $commit THEN $request_id ELSE commit_request_id END,
    phase = CASE WHEN $commit THEN 'committing' ELSE phase END,
    updated_at = now()
WHERE id = $id AND task_instance_id = $task_instance_id
  AND released_at IS NULL AND phase IN ('active', 'committing')
  AND (NOT $commit OR commit_request_id IS NULL)
  AND EXISTS (SELECT 1 FROM public.task_instances t
    WHERE t.id = remote_attempts.task_instance_id AND t.live IS TRUE AND t.state = 'running')
