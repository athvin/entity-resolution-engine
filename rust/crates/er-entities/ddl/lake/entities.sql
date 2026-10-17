CREATE TABLE IF NOT EXISTS lake.main.entities (
  entity_id VARCHAR NOT NULL,
  status VARCHAR NOT NULL,
  merged_into VARCHAR,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL,
  created_run_id VARCHAR NOT NULL,
  updated_run_id VARCHAR NOT NULL
);
