UPDATE public.api_keys SET revoked_at = now()
WHERE key_id = $key_id AND org = $org AND revoked_at IS NULL
