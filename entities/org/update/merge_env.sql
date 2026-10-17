UPDATE public.orgs SET env = env || $updates WHERE name = $name
