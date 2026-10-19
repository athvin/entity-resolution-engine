CREATE TABLE IF NOT EXISTS lake.main.golden_overrides (
  id VARCHAR NOT NULL,
  entity_id VARCHAR NOT NULL,
  attribute VARCHAR NOT NULL,
  value VARCHAR,
  created_by VARCHAR NOT NULL,
  created_at TIMESTAMP NOT NULL,
  source_action_id VARCHAR NOT NULL,
  updated_at TIMESTAMP NOT NULL
);
