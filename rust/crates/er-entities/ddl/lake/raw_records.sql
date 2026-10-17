CREATE TABLE IF NOT EXISTS lake.main.raw_records (
  source_system VARCHAR NOT NULL,
  source_record_id VARCHAR NOT NULL,
  payload JSON,
  content_hash VARCHAR NOT NULL,
  is_deleted BOOLEAN NOT NULL,
  deleted_at TIMESTAMP,
  ingest_batch_id VARCHAR NOT NULL,
  ingested_at TIMESTAMP NOT NULL
);
