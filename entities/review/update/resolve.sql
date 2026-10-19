UPDATE lake.main.review_queue SET status = $status, resolved_at = $resolved_at::timestamp,
    updated_at = $resolved_at::timestamp
WHERE id = $review_id
