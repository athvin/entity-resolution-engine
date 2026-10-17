CREATE TABLE IF NOT EXISTS lake.main.match_scores (
  rec_a_key VARCHAR NOT NULL,
  rec_b_key VARCHAR NOT NULL,
  match_probability DOUBLE NOT NULL,
  model_version VARCHAR NOT NULL,
  tf_snapshot_id VARCHAR NOT NULL,
  rec_a_content_hash VARCHAR NOT NULL,
  rec_b_content_hash VARCHAR NOT NULL,
  evidence JSON NOT NULL,
  is_active BOOLEAN NOT NULL,
  invalidated_at TIMESTAMP,
  invalidated_run_id VARCHAR,
  run_id VARCHAR NOT NULL,
  scored_at TIMESTAMP NOT NULL
);
