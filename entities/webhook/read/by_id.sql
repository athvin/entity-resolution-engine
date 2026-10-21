SELECT id, org_id, url, events, secret, active, created_at, updated_at
FROM public.webhooks
WHERE id = $webhook_id AND active
