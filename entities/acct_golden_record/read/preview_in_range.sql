SELECT id, company_name, domain
FROM lake.main.acct_golden_records AT (VERSION => $snapshot)
WHERE id >= $first AND id <= $last
