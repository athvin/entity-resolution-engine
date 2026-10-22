SELECT id, created_at, updated_at, catalog_hash, kind, relation, statement, applied_by
FROM public.schema_migrations
ORDER BY created_at DESC, id DESC
LIMIT 1
