SELECT
  entity_id,
  company_name,
  domain,
  phone_e164,
  addr_street,
  addr_city,
  addr_region,
  addr_postal,
  metadata,
  survivorship_version,
  assembled_at
FROM lake.main.acct_golden_records AT (VERSION => $snapshot)
WHERE entity_id = $entity_id
