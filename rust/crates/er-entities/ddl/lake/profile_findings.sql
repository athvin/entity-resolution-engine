CREATE TABLE IF NOT EXISTS lake.main.profile_findings (
  id VARCHAR NOT NULL,
  profile_run_id VARCHAR NOT NULL,
  code VARCHAR NOT NULL,
  column_name VARCHAR NOT NULL,
  severity VARCHAR NOT NULL,
  message VARCHAR NOT NULL,
  detail JSON,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);
