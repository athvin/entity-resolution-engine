CREATE TABLE IF NOT EXISTS lake.main.acct_golden_lineage (
  id VARCHAR NOT NULL,
  entity_id VARCHAR NOT NULL,
  attribute VARCHAR NOT NULL,
  record_key VARCHAR NOT NULL,
  source_system VARCHAR NOT NULL,
  source_record_id VARCHAR NOT NULL,
  rule VARCHAR NOT NULL,
  survivorship_version VARCHAR NOT NULL,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);
