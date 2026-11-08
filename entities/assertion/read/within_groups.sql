WITH members AS (
  SELECT entity_id, record_key
  FROM lake.main.entity_membership AT (VERSION => $snapshot)
  WHERE entity_id >= $first AND entity_id <= $last
),
asserted AS (
  SELECT rec_a_key, rec_b_key
  FROM lake.main.assertions AT (VERSION => $snapshot)
  WHERE kind = 'always' AND active
)
SELECT a.entity_id, asserted.rec_a_key, asserted.rec_b_key, CAST(1.0 AS DOUBLE) AS match_probability
FROM asserted
JOIN members a ON a.record_key = asserted.rec_a_key
JOIN members b ON b.record_key = asserted.rec_b_key AND b.entity_id = a.entity_id
ORDER BY entity_id, rec_a_key, rec_b_key
