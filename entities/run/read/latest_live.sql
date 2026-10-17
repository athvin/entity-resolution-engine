SELECT run_id, status, mode, model_version, tf_snapshot_id
FROM lake.main.runs
ORDER BY started_at DESC, run_id DESC
LIMIT 1
