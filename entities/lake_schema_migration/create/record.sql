INSERT INTO lake.main.schema_migrations (id, created_at, updated_at, catalog_hash, kind, relation,
  statement, applied_by, snapshot_id)
VALUES ($id, $created_at::timestamp, $created_at::timestamp, $catalog_hash, $kind, $relation,
  $statement, $applied_by, $snapshot_id)
