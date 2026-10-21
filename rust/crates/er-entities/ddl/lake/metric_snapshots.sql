CREATE TABLE IF NOT EXISTS lake.main.metric_snapshots (
  id VARCHAR NOT NULL,
  run_id VARCHAR NOT NULL,
  captured_at TIMESTAMP NOT NULL,
  records BIGINT NOT NULL,
  entities BIGINT NOT NULL,
  duplicate_records BIGINT NOT NULL,
  duplicate_groups BIGINT NOT NULL,
  largest_group BIGINT NOT NULL,
  review_backlog BIGINT NOT NULL,
  auto_merge_pairs BIGINT NOT NULL,
  review_band_pairs BIGINT NOT NULL,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);
