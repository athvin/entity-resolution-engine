SELECT catalog_hash, kind, created_at
FROM lake.main.schema_migrations
ORDER BY created_at DESC, id DESC
LIMIT 1
