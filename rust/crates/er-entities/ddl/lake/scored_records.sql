CREATE TABLE IF NOT EXISTS lake.main.scored_records (
  id VARCHAR NOT NULL,
  record_key VARCHAR NOT NULL,
  content_hash VARCHAR NOT NULL,
  model_version VARCHAR NOT NULL,
  tf_snapshot_id VARCHAR NOT NULL,
  run_id VARCHAR NOT NULL,
  scored_at TIMESTAMP NOT NULL,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);
