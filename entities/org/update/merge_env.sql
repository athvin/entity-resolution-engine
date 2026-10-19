UPDATE public.orgs SET env = env || $updates, updated_at = now() WHERE name = $name
