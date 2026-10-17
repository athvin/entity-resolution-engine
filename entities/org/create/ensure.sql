INSERT INTO public.orgs (name, config_path, state)
VALUES ($name, $config_path, 'active')
ON CONFLICT (name) DO NOTHING
