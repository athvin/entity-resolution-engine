CREATE TABLE IF NOT EXISTS lake.main.acct_golden_records (
  id VARCHAR NOT NULL,
  company_name VARCHAR,
  domain VARCHAR,
  phone_e164 VARCHAR,
  addr_street VARCHAR,
  addr_city VARCHAR,
  addr_region VARCHAR,
  addr_postal VARCHAR,
  metadata JSON,
  survivorship_version VARCHAR NOT NULL,
  assembled_at TIMESTAMP NOT NULL,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);
