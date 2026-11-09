SELECT resource_class, count(*) AS depth
FROM public.jobs
WHERE state = 'queued'
GROUP BY resource_class
