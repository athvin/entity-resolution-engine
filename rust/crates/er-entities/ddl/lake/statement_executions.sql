CREATE TABLE IF NOT EXISTS lake.main.statement_executions (
  id VARCHAR NOT NULL,
  run_id VARCHAR NOT NULL,
  stage VARCHAR NOT NULL,
  seq BIGINT NOT NULL,
  statement VARCHAR NOT NULL,
  kind VARCHAR NOT NULL,
  store VARCHAR NOT NULL,
  relation VARCHAR,
  duration_ms BIGINT NOT NULL,
  row_count BIGINT,
  outcome VARCHAR NOT NULL,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);
