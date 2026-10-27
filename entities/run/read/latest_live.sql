SELECT id, status, mode, model_version, tf_snapshot_id, resolved_ensemble
FROM lake.main.runs
ORDER BY started_at DESC, id DESC
LIMIT 1
