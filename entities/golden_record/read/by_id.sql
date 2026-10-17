SELECT entity_id, given_name, family_name, email, phone_e164,
       addr_number, addr_street, addr_unit, addr_city, addr_region, addr_postal,
       birth_date, metadata, survivorship_version
FROM lake.main.golden_records AT (VERSION => $snapshot)
WHERE entity_id = $entity_id
