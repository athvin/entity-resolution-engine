INSERT INTO lake.main.assertions
  (id, rec_a_key, rec_b_key, kind, active, created_by, created_at, updated_at)
SELECT $assertion_id, rec_a_key, rec_b_key, $kind, true, 'steward', $created_at::timestamp,
  $created_at::timestamp
FROM lake.main.review_queue WHERE id = $review_id
