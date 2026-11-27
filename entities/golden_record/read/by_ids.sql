SELECT id, given_name, family_name, email, phone_e164, addr_number, addr_street, addr_unit,
       addr_city, addr_region, addr_postal, birth_date
FROM lake.main.golden_records AT (VERSION => $snapshot)
WHERE id IN (SELECT unnest(string_split($ids, chr(31))))
ORDER BY id
