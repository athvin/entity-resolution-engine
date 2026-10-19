CREATE TABLE IF NOT EXISTS lake.main.record_overlays (
  id VARCHAR NOT NULL,
  record_key VARCHAR NOT NULL,
  kind VARCHAR NOT NULL,
  active BOOLEAN NOT NULL,
  created_by VARCHAR NOT NULL,
  created_at TIMESTAMP NOT NULL,
  source_action_id VARCHAR NOT NULL,
  retracted_at TIMESTAMP,
  updated_at TIMESTAMP NOT NULL
);
