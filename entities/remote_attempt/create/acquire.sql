INSERT INTO public.remote_attempts
  (id, org_id, dataset_id, job_id, task_instance_id, launch,
   binding_generation, namespace, phase)
SELECT $id, t.org_id, t.dataset_id, t.job_id, t.id, t.launches,
       $binding_generation, $namespace, 'opening'
FROM public.task_instances t JOIN public.jobs j ON j.id = t.job_id
WHERE t.id = $task_instance_id AND t.pod_uid = $pod_uid
  AND t.live IS TRUE AND t.state = 'running' AND j.state = 'running'
  AND t.dataset_id IS NOT NULL
ON CONFLICT DO NOTHING
