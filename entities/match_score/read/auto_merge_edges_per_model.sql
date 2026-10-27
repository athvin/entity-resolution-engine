SELECT m.rec_a_key, m.rec_b_key, m.match_probability
FROM lake.main.match_scores m
JOIN __member_thresholds t
  ON t.model_version = m.model_version
 AND t.tf_snapshot_id = m.tf_snapshot_id
WHERE m.is_active AND m.match_probability >= t.threshold
ORDER BY m.rec_a_key, m.rec_b_key
