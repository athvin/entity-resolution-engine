CREATE TABLE IF NOT EXISTS lake.main.assertions (
  assertion_id VARCHAR NOT NULL,
  rec_a_key VARCHAR NOT NULL,
  rec_b_key VARCHAR NOT NULL,
  kind VARCHAR NOT NULL,
  active BOOLEAN NOT NULL,
  created_by VARCHAR NOT NULL,
  created_at TIMESTAMP NOT NULL,
  retracted_by VARCHAR,
  retracted_at TIMESTAMP,
  note VARCHAR
);
