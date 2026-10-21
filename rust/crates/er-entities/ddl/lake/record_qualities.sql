CREATE TABLE IF NOT EXISTS lake.main.record_qualities (
  id VARCHAR NOT NULL,
  record_key VARCHAR NOT NULL,
  score DOUBLE NOT NULL,
  completeness DOUBLE NOT NULL,
  validity DOUBLE,
  freshness DOUBLE,
  run_id VARCHAR NOT NULL,
  scored_at TIMESTAMP NOT NULL,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);
