SELECT id, given_name, family_name, email
FROM lake.main.golden_records AT (VERSION => $snapshot)
WHERE id >= $first AND id <= $last
