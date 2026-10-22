UPDATE lake.main.ingest_batches
   SET matched_count = $matched_count,
       review_count = $review_count,
       unmatched_count = $unmatched_count,
       screened_at = $screened_at::timestamp,
       updated_at = $screened_at::timestamp
 WHERE id = $ingest_batch_id
