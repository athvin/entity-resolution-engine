UPDATE public.salesforce_connections
SET active = false, updated_at = now()
WHERE org_id = $org_id AND id = $connection_id AND active
