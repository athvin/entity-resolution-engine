UPDATE lake.main.run_stages
SET status = $status, ended_at = $ended_at::timestamp, snapshot_end = $snapshot_end, counters = $counters::json,
    error_class = $error_class, error_detail = $error_detail, updated_at = $ended_at::timestamp
WHERE id = $id
