SELECT id, org_id, catalog_hash, status, detail, checked_at, created_at, updated_at
FROM public.tenant_schema_states
ORDER BY org_id
