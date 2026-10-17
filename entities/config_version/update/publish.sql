UPDATE public.config_versions SET state = 'published', tier = $tier, published_at = now()
WHERE org = $org AND version = $version
