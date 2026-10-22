SELECT rec_a_key, rec_b_key
FROM lake.main.review_queue
WHERE id = $review_id
