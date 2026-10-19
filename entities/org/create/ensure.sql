INSERT INTO public.orgs (id, name, config_path, state)
VALUES ($id, $name, $config_path, 'active')
ON CONFLICT (name) DO NOTHING
