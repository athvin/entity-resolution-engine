INSERT INTO public.merge_preview_watermarks
  (id, org_id, dataset_id, run_snapshot_end, run_id, lake_snapshot, job_id)
SELECT $id, $org_id, $dataset_id, $run_snapshot_end, $run_id, $lake_snapshot, $job_id
WHERE $expected_run_id = ''
ON CONFLICT (dataset_id) DO NOTHING
RETURNING id, org_id, dataset_id, run_snapshot_end, run_id, lake_snapshot, job_id, created_at, updated_at
