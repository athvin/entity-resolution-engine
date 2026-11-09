SELECT resource_class, count(*) AS active
FROM public.task_instances
WHERE live IS TRUE AND kind = 'pod'
  AND state IN ('queued', 'running', 'restarting', 'canceling')
GROUP BY resource_class
