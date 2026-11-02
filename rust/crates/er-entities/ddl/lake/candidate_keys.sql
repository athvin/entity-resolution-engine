CREATE TABLE IF NOT EXISTS lake.main.candidate_keys (
  id VARCHAR NOT NULL,
  profile_run_id VARCHAR NOT NULL,
  key_columns VARCHAR NOT NULL,
  row_count BIGINT NOT NULL,
  covered_rows BIGINT NOT NULL,
  distinct_values BIGINT NOT NULL,
  expected_pairs BIGINT NOT NULL,
  largest_block_pairs BIGINT NOT NULL,
  largest_block_rows BIGINT NOT NULL,
  entropy_bits DOUBLE NOT NULL,
  pair_loss_from_nulls DOUBLE NOT NULL,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);
