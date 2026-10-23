UPDATE public.webhooks
SET active = false, updated_at = now()
WHERE org_id = $org_id AND id = $webhook_id AND active
