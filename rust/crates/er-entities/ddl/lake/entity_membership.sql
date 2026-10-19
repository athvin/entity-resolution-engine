CREATE TABLE IF NOT EXISTS lake.main.entity_membership (
  id VARCHAR NOT NULL,
  source_system VARCHAR NOT NULL,
  source_record_id VARCHAR NOT NULL,
  record_key VARCHAR NOT NULL,
  entity_id VARCHAR NOT NULL,
  assigned_at TIMESTAMP NOT NULL,
  run_id VARCHAR NOT NULL,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);
