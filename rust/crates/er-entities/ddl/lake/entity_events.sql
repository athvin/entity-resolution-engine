CREATE TABLE IF NOT EXISTS lake.main.entity_events (
  event_id VARCHAR NOT NULL,
  seq BIGINT NOT NULL,
  run_id VARCHAR NOT NULL,
  entity_id VARCHAR NOT NULL,
  event_type VARCHAR NOT NULL,
  details JSON NOT NULL,
  details_hash VARCHAR NOT NULL,
  occurred_at TIMESTAMP NOT NULL,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);
