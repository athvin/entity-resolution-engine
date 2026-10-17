SELECT assertion_id, rec_a_key, rec_b_key, kind
FROM lake.main.assertions AT (VERSION => $snapshot)
WHERE active
