SELECT resource_class, count(*) AS depth, min(created_at) AS oldest_created_at
FROM public.jobs
WHERE state = 'queued'
GROUP BY resource_class
