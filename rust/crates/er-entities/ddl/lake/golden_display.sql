CREATE TABLE IF NOT EXISTS lake.main.golden_display (
  entity_id VARCHAR NOT NULL,
  display_name VARCHAR,
  display_email VARCHAR,
  display_phone VARCHAR,
  display_address VARCHAR,
  assembled_at TIMESTAMP NOT NULL
);
