WITH members AS (
  SELECT entity_id, record_key
  FROM lake.main.entity_membership AT (VERSION => $snapshot)
  WHERE entity_id >= $first AND entity_id <= $last
),
scored AS (
  SELECT rec_a_key, rec_b_key, match_probability
  FROM lake.main.match_scores AT (VERSION => $snapshot)
  WHERE is_active
)
SELECT a.entity_id, scored.rec_a_key, scored.rec_b_key, scored.match_probability
FROM scored
JOIN members a ON a.record_key = scored.rec_a_key
JOIN members b ON b.record_key = scored.rec_b_key AND b.entity_id = a.entity_id
ORDER BY entity_id, rec_a_key, rec_b_key
