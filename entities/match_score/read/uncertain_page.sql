SELECT rec_a_key, rec_b_key, match_probability, model_version, evidence, run_id, scored_at
FROM lake.main.match_scores AT (VERSION => $snapshot)
WHERE is_active
  AND match_probability >= $band_low
  AND match_probability <= $band_high
-- Distance from the threshold, written as a CASE rather than abs(): DuckDB has
-- abs, but the entity analyser's planner does not, and a statement that cannot
-- be planned cannot be declared.
ORDER BY (CASE WHEN match_probability >= $threshold
               THEN match_probability - $threshold
               ELSE $threshold - match_probability END),
         rec_a_key, rec_b_key
LIMIT $limit
