UPDATE public.schedules c
SET dataset_id = d.id, updated_at = now()
FROM public.datasets d
WHERE d.org_id = c.org_id AND d.name = 'default' AND c.dataset_id IS NULL
