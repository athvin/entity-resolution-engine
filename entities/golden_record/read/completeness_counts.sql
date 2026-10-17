SELECT count(*) AS n,
       count(given_name) AS c_given_name,
       count(family_name) AS c_family_name,
       count(email) AS c_email,
       count(phone_e164) AS c_phone_e164,
       count(addr_number) AS c_addr_number,
       count(addr_street) AS c_addr_street,
       count(addr_unit) AS c_addr_unit,
       count(addr_city) AS c_addr_city,
       count(addr_region) AS c_addr_region,
       count(addr_postal) AS c_addr_postal,
       count(birth_date) AS c_birth_date
FROM lake.main.golden_records AT (VERSION => $snapshot)
