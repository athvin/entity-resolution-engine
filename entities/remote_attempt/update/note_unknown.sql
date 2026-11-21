UPDATE public.remote_attempts
SET phase = 'unknown', updated_at = now()
WHERE id = $id AND task_instance_id = $task_instance_id
  AND released_at IS NULL AND phase IN ('opening', 'active', 'committing')
