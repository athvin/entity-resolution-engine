CREATE TABLE IF NOT EXISTS lake.main.ingest_batches (
  id VARCHAR NOT NULL,
  run_id VARCHAR NOT NULL,
  source_system VARCHAR NOT NULL,
  path VARCHAR NOT NULL,
  new_count BIGINT NOT NULL,
  changed_count BIGINT NOT NULL,
  unchanged_count BIGINT NOT NULL,
  tombstone_count BIGINT NOT NULL,
  resurrected_count BIGINT NOT NULL,
  full_refresh_keys BOOLEAN NOT NULL,
  matched_count BIGINT,
  review_count BIGINT,
  unmatched_count BIGINT,
  screened_at TIMESTAMP,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);
