SELECT
  name,
  config_path,
  env,
  drop_root,
  active_config_version,
  state,
  created_at,
  updated_at
FROM public.orgs
WHERE name = $name
