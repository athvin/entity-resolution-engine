SELECT rec_a_key, rec_b_key, match_probability, model_version, tf_snapshot_id
FROM lake.main.match_scores
WHERE is_active AND match_probability >= $min_threshold
ORDER BY rec_a_key, rec_b_key
