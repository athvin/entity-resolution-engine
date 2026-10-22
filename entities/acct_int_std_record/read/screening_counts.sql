WITH delivered AS (
  SELECT record_key
  FROM lake.main.acct_int_std_records
  WHERE source_system = $source AND ingest_batch_id = $batch
),
endpoints AS (
  SELECT rec_a_key AS record_key, match_probability
  FROM lake.main.match_scores
  WHERE is_active
  UNION ALL
  SELECT rec_b_key AS record_key, match_probability
  FROM lake.main.match_scores
  WHERE is_active
),
verdict AS (
  SELECT d.record_key, max(e.match_probability) AS best
  FROM delivered d
  LEFT JOIN endpoints e ON e.record_key = d.record_key
  GROUP BY d.record_key
)
SELECT
  count(*) FILTER (WHERE best >= $auto_merge) AS matched_count,
  count(*) FILTER (WHERE best >= $review_low AND best < $auto_merge) AS review_count,
  count(*) FILTER (WHERE best IS NULL OR best < $review_low) AS unmatched_count
FROM verdict
