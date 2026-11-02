UPDATE public.datasets SET env = env || $patch, updated_at = now()
WHERE org_id = $org_id
