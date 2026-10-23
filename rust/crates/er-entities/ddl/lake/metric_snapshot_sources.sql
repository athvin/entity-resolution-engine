CREATE TABLE IF NOT EXISTS lake.main.metric_snapshot_sources (
  id VARCHAR NOT NULL,
  run_id VARCHAR NOT NULL,
  source_system VARCHAR NOT NULL,
  captured_at TIMESTAMP NOT NULL,
  records BIGINT NOT NULL,
  duplicate_records BIGINT NOT NULL,
  contributed_groups BIGINT NOT NULL,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);
