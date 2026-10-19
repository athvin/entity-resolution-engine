SELECT
  id,
  company_name,
  domain,
  phone_e164,
  addr_street,
  addr_city,
  addr_region,
  addr_postal,
  metadata,
  survivorship_version,
  assembled_at,
  created_at,
  updated_at
FROM lake.main.acct_golden_records AT (VERSION => $snapshot)
WHERE ($after IS NULL OR id > $after)
  AND (
    $q IS NULL
    OR (coalesce(company_name, '') || ' ' || coalesce(domain, '')) ILIKE $q ESCAPE '\'
  )
ORDER BY id
LIMIT $limit
