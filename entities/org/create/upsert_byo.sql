INSERT INTO public.orgs (name, config_path, env, drop_root, state)
VALUES ($name, $config_path, $env, $drop_root, 'active')
ON CONFLICT (name) DO UPDATE SET config_path = EXCLUDED.config_path,
  env = EXCLUDED.env, drop_root = EXCLUDED.drop_root
