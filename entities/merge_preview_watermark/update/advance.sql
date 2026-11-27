UPDATE public.merge_preview_watermarks
SET run_snapshot_end = $run_snapshot_end, run_id = $run_id, lake_snapshot = $lake_snapshot,
    job_id = $job_id, updated_at = now()
WHERE org_id = $org_id AND dataset_id = $dataset_id
  AND run_snapshot_end = $expected_snapshot_end AND run_id = $expected_run_id
RETURNING id, org_id, dataset_id, run_snapshot_end, run_id, lake_snapshot, job_id, created_at, updated_at
