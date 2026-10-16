SELECT
  entity_id,
  given_name,
  family_name,
  email,
  phone_e164,
  addr_number,
  addr_street,
  addr_unit,
  addr_city,
  addr_region,
  addr_postal,
  birth_date,
  metadata,
  survivorship_version,
  assembled_at
FROM lake.main.golden_records AT (VERSION => $snapshot)
WHERE ($after IS NULL OR entity_id > $after)
  AND (
    $q IS NULL
    OR (coalesce(given_name, '') || ' ' || coalesce(family_name, '') || ' ' || coalesce(email, '')) ILIKE $q ESCAPE '\'
  )
ORDER BY entity_id
LIMIT $limit
