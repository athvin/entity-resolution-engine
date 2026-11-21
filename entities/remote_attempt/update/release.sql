UPDATE public.remote_attempts
SET phase = $phase, released_at = now(), updated_at = now()
WHERE id = $id AND task_instance_id = $task_instance_id
  AND released_at IS NULL
  AND $phase IN ('committed', 'rolled_back', 'not_executed')
