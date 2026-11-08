CREATE TABLE IF NOT EXISTS lake.main.golden_record_overrides (
  id VARCHAR NOT NULL,
  record_key VARCHAR NOT NULL,
  attribute VARCHAR NOT NULL,
  value VARCHAR,
  created_by VARCHAR NOT NULL,
  created_at TIMESTAMP NOT NULL,
  source_action_id VARCHAR NOT NULL,
  updated_at TIMESTAMP NOT NULL
);
