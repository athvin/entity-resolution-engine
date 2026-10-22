CREATE TABLE IF NOT EXISTS lake.main.schema_migrations (
  id VARCHAR NOT NULL,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL,
  catalog_hash VARCHAR NOT NULL,
  kind VARCHAR NOT NULL,
  relation VARCHAR,
  statement VARCHAR,
  applied_by VARCHAR NOT NULL,
  snapshot_id BIGINT
);
