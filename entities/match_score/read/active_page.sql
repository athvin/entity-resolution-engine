SELECT rec_a_key, rec_b_key, match_probability, model_version, evidence, run_id, scored_at
FROM lake.main.match_scores AT (VERSION => $snapshot)
WHERE is_active
  AND ($after_a IS NULL OR rec_a_key > $after_a
       OR (rec_a_key = $after_a AND rec_b_key > $after_b))
  AND ($min_prob IS NULL OR match_probability >= $min_prob)
  AND ($max_prob IS NULL OR match_probability <= $max_prob)
ORDER BY rec_a_key, rec_b_key
LIMIT $limit
