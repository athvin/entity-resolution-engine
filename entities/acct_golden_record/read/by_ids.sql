SELECT id, company_name, domain, phone_e164, addr_street, addr_city, addr_region, addr_postal
FROM lake.main.acct_golden_records AT (VERSION => $snapshot)
WHERE id IN (SELECT unnest(string_split($ids, chr(31))))
ORDER BY id
