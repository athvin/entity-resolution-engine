UPDATE public.api_keys SET revoked_at = now(), updated_at = now()
WHERE id = $key_id AND org_id = $org_id AND revoked_at IS NULL
