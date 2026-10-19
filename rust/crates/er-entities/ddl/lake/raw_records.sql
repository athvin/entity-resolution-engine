CREATE TABLE IF NOT EXISTS lake.main.raw_records (
  id VARCHAR NOT NULL,
  source_system VARCHAR NOT NULL,
  source_record_id VARCHAR NOT NULL,
  payload JSON,
  content_hash VARCHAR NOT NULL,
  is_deleted BOOLEAN NOT NULL,
  deleted_at TIMESTAMP,
  ingest_batch_id VARCHAR NOT NULL,
  ingested_at TIMESTAMP NOT NULL,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);
