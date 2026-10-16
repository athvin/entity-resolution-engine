CREATE TABLE IF NOT EXISTS lake.main.golden_records (
  entity_id VARCHAR NOT NULL,
  given_name VARCHAR,
  family_name VARCHAR,
  email VARCHAR,
  phone_e164 VARCHAR,
  addr_number VARCHAR,
  addr_street VARCHAR,
  addr_unit VARCHAR,
  addr_city VARCHAR,
  addr_region VARCHAR,
  addr_postal VARCHAR,
  birth_date DATE,
  metadata JSON,
  survivorship_version VARCHAR NOT NULL,
  assembled_at TIMESTAMP NOT NULL
);
