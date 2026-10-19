CREATE TABLE IF NOT EXISTS lake.main.review_queue (
  review_id VARCHAR NOT NULL,
  subject_type VARCHAR NOT NULL,
  rec_a_key VARCHAR,
  rec_b_key VARCHAR,
  entity_id VARCHAR,
  reason VARCHAR NOT NULL,
  match_probability DOUBLE,
  waterfall JSON,
  status VARCHAR NOT NULL,
  first_seen_run_id VARCHAR NOT NULL,
  last_seen_run_id VARCHAR NOT NULL,
  resolved_by VARCHAR,
  resolved_at TIMESTAMP,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);
