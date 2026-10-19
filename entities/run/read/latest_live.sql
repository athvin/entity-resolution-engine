SELECT id, status, mode, model_version, tf_snapshot_id
FROM lake.main.runs
ORDER BY started_at DESC, id DESC
LIMIT 1
