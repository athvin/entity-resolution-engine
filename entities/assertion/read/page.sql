SELECT id, rec_a_key, rec_b_key, kind, active, created_by, created_at,
       retracted_by, retracted_at, note
FROM lake.main.assertions AT (VERSION => $snapshot)
WHERE ($include_retracted OR active) AND ($after IS NULL OR id > $after)
ORDER BY id
LIMIT $limit
