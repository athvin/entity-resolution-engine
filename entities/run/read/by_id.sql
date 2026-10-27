SELECT id, model_version, tf_snapshot_id, resolved_ensemble
FROM lake.main.runs
WHERE id = $run_id
