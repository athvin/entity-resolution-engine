SELECT entity_id, company_name, domain
FROM lake.main.acct_golden_records AT (VERSION => $snapshot)
WHERE entity_id >= $first AND entity_id <= $last
