UPDATE public.config_versions SET state = 'published', tier = $tier, published_at = now(),
    updated_at = now()
WHERE org_id = $org_id AND version = $version
