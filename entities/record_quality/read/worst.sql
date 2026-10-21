SELECT id, record_key, score, completeness, validity, freshness, run_id, scored_at,
       created_at, updated_at
FROM lake.main.record_qualities AT (VERSION => $snapshot)
WHERE score <= $max_score
ORDER BY score, record_key
LIMIT $limit
