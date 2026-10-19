INSERT INTO public.orgs (id, name, config_path, env, drop_root, state)
VALUES ($id, $name, $config_path, $env, $drop_root, 'active')
ON CONFLICT (name) DO UPDATE SET config_path = EXCLUDED.config_path,
  env = EXCLUDED.env, drop_root = EXCLUDED.drop_root, updated_at = now()
