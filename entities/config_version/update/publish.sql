UPDATE public.config_versions SET state = 'published', tier = $tier, published_at = now(),
    updated_at = now()
WHERE dataset_id = $dataset_id AND version = $version
