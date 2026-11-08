WITH members AS (
  SELECT entity_id, record_key
  FROM lake.main.entity_membership AT (VERSION => $snapshot)
  WHERE entity_id >= $first AND entity_id <= $last
)
SELECT a.entity_id, ms.rec_a_key, ms.rec_b_key, ms.match_probability
FROM lake.main.match_scores AT (VERSION => $snapshot) ms
JOIN members a ON a.record_key = ms.rec_a_key
JOIN members b ON b.record_key = ms.rec_b_key AND b.entity_id = a.entity_id
WHERE ms.is_active
UNION ALL
SELECT a.entity_id, asn.rec_a_key, asn.rec_b_key, CAST(1.0 AS DOUBLE) AS match_probability
FROM lake.main.assertions AT (VERSION => $snapshot) asn
JOIN members a ON a.record_key = asn.rec_a_key
JOIN members b ON b.record_key = asn.rec_b_key AND b.entity_id = a.entity_id
WHERE asn.kind = 'always' AND asn.active
ORDER BY entity_id, rec_a_key, rec_b_key
