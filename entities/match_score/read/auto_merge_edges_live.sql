SELECT rec_a_key, rec_b_key, match_probability
FROM lake.main.match_scores
WHERE is_active AND match_probability >= $threshold
ORDER BY rec_a_key, rec_b_key
