UPDATE public.orgs SET active_config_version = $version, updated_at = now() WHERE name = $name
