SELECT id, org_id, dataset_id, job_id, task_id, resource_class, node_name, state,
       queued_at, started_at, finished_at
FROM public.task_instances
WHERE kind = 'pod' AND node_name IS NOT NULL AND started_at IS NOT NULL
  AND finished_at IS NOT NULL
  AND finished_at >= $from_ts AND finished_at < $to_ts
ORDER BY finished_at, id
