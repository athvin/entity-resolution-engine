UPDATE public.datasets SET active_config_version = $version, updated_at = now()
WHERE id = $dataset_id
