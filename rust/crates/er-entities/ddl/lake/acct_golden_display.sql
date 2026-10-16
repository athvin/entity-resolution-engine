CREATE TABLE IF NOT EXISTS lake.main.acct_golden_display (
  entity_id VARCHAR NOT NULL,
  display_name VARCHAR,
  display_domain VARCHAR,
  display_phone VARCHAR,
  display_address VARCHAR,
  assembled_at TIMESTAMP NOT NULL
);
