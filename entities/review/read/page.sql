SELECT id, subject_type, rec_a_key, rec_b_key, entity_id, reason,
       match_probability, waterfall, status, first_seen_run_id, last_seen_run_id,
       resolved_by, resolved_at
FROM lake.main.review_queue AT (VERSION => $snapshot)
WHERE ($after IS NULL OR id > $after)
  AND ($status IS NULL OR status = $status)
  AND ($reason IS NULL OR reason = $reason)
  AND (($min_probability IS NULL AND $max_probability IS NULL)
       OR (match_probability IS NOT NULL
           AND ($min_probability IS NULL OR match_probability >= $min_probability)
           AND ($max_probability IS NULL OR match_probability <= $max_probability)))
ORDER BY id
LIMIT $limit
