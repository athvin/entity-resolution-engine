SELECT record_key, source_system, source_record_id,
       given_name, family_name, name_variants, email, email_valid,
       phone_e164, phone_valid, addr_number, addr_street, addr_unit,
       addr_city, addr_region, addr_postal, birth_date, metadata,
       updated_at_source
FROM lake.main.int_std_records AT (VERSION => $snapshot)
WHERE record_key IN (SELECT unnest(string_split($keys, chr(31))))
ORDER BY record_key
