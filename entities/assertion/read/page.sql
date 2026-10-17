SELECT assertion_id, rec_a_key, rec_b_key, kind, active, created_by, created_at,
       retracted_by, retracted_at, note
FROM lake.main.assertions AT (VERSION => $snapshot)
WHERE ($include_retracted OR active) AND ($after IS NULL OR assertion_id > $after)
ORDER BY assertion_id
LIMIT $limit
